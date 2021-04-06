#![cfg_attr(not(test), no_std)]

use core::{any::Any, fmt::Debug, marker::PhantomData};

extern crate alloc;
use crate::alloc::borrow::ToOwned;
use crate::alloc::{boxed::Box, sync::Arc};

pub mod channel;
use channel::{Receiver, Sender, TrySendError};

use log::{debug, warn};

/// Any message that can be sent and received by an actor. Used within dispatchers.
pub type AnyMessage = Box<dyn Any + Send>;

/// An actor is a computational entity that, in response to a message it receives, can concurrently:
/// * send messages to other actors
/// * create new actors
/// * designate the behavior to be used for the next message it receives
/// (from https://en.wikipedia.org/wiki/Actor_model#Fundamental_concepts)
pub trait Actor<M> {
    /// Receive a message while being able to mutate the actor state safely and
    /// without requiring a locking mechanism. An actor context is the meta state
    /// that enables actor creation and the stopping of this actor.
    fn receive(&mut self, context: &mut ActorContext<M>, message: &M);
}

/// This is the type used to represent its corresponding enum as enum variants cannot
/// be used as types in Rust.
pub struct SelectWithAction {
    pub receiver: Receiver<AnyMessage>,
    pub action: Box<dyn FnMut(AnyMessage) -> bool + Send + Sync>,
}

/// Dispatchers can be sent commands on a control channel as well as being able to
/// dispatch messages to the actors they are responsible to execute.
pub enum DispatcherCommand {
    /// Tells the dispatcher to select on a receiver of messages by providing
    /// the receiver. If selection signals activity on the receiver then
    /// a function should be performed to process it.
    SelectWithAction { underlying: SelectWithAction },

    /// Tells the dispatcher to finish up. The thread on which the dispatcher run
    /// function is running can then be joined.
    Stop,
}

/// A dispatcher composes a executor to call upon the actor's message queue, ultimately calling
/// upon the actor's receive method.
pub trait Dispatcher {
    /// Enqueue a command to the channel being selected on.
    fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>>;

    /// Stop the current dispatcher
    fn stop(&self) {
        let _ = self.send(DispatcherCommand::Stop);
    }
}

/// An actor context provides state that all actors need to be able to operate.
/// These contexts are used mainly to obtain actor references to themselves.
pub struct ActorContext<M> {
    active: bool,
    pub actor_ref: ActorRef<M>,
    pub dispatcher: Arc<dyn Dispatcher + Send + Sync>,
    pub mailbox_fn: Arc<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync>,
}

impl<M> ActorContext<M> {
    /// Create a new actor context and associate it with a dispatcher.
    pub fn new<FA>(
        new_actor_fn: FA,
        dispatcher: Arc<dyn Dispatcher + Send + Sync>,
        mailbox_fn: Arc<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync>,
    ) -> ActorContext<M>
    where
        FA: FnOnce() -> Box<dyn Actor<M> + Send + Sync>,
        M: Send + Sync + 'static,
    {
        let (tx, rx) = mailbox_fn();
        let actor_ref = ActorRef {
            phantom_marker: PhantomData,
            sender: tx,
        };
        let context = ActorContext {
            active: true,
            actor_ref: actor_ref.to_owned(),
            dispatcher: dispatcher.to_owned(),
            mailbox_fn,
        };
        let mut actor = new_actor_fn();

        let mut dispatcher_context = context.to_owned();

        if let Err(e) = dispatcher.send(DispatcherCommand::SelectWithAction {
            underlying: SelectWithAction {
                receiver: rx,
                action: Box::new(move |message| {
                    if dispatcher_context.active {
                        match message.downcast::<M>() {
                            Ok(ref m) => {
                                actor.receive(&mut dispatcher_context, m);
                            }
                            Err(m) => warn!(
                                "Unexpected message in {:?}: type_id: {:?}",
                                dispatcher_context.actor_ref,
                                m.type_id()
                            ),
                        }
                    }
                    dispatcher_context.active
                }),
            },
        }) {
            debug!("Error received establishing {:?}: {}", actor_ref, e);
        }

        context
    }

    /// Create a new actor as a child to this one. The child actor will receive
    /// the same dispatcher as the current one.
    pub fn spawn<FA, M2>(&mut self, new_actor_fn: FA) -> ActorRef<M2>
    where
        FA: FnOnce() -> Box<dyn Actor<M2> + Send + Sync>,
        M2: Send + Sync + 'static,
    {
        let context = ActorContext::<M2>::new(
            new_actor_fn,
            self.dispatcher.to_owned(),
            self.mailbox_fn.to_owned(),
        );
        context.actor_ref
    }

    /// Stop this actor immediately.
    pub fn stop(&mut self) {
        self.active = true;
    }
}

impl<M> Clone for ActorContext<M> {
    fn clone(&self) -> ActorContext<M> {
        ActorContext {
            active: self.active,
            actor_ref: self.actor_ref.to_owned(),
            dispatcher: self.dispatcher.to_owned(),
            mailbox_fn: self.mailbox_fn.to_owned(),
        }
    }
}

/// An actor ref provides a means by which to communicate
/// with an actor; in fact it is the only means to send
/// a message to an actor. Any associated actor may no
/// longer exist, in which case messages will be delivered
/// to a dead letter channel.
pub struct ActorRef<M> {
    phantom_marker: PhantomData<M>,
    sender: Sender<AnyMessage>,
}

impl<M: Send + 'static> ActorRef<M> {
    /// Perform an ask operation on the associated actor
    /// while passing in a function to construct a message
    /// that accepts a reply_to sender
    pub fn ask<M2>(
        &self,
        request_fn: &dyn Fn(&ActorRef<M2>) -> M,
        mailbox_fn: Arc<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>)>,
    ) -> Receiver<AnyMessage>
    where
        M2: Send + 'static,
    {
        let (reply_tx, reply_rx) = mailbox_fn();
        let _ = self.sender.try_send(Box::new(request_fn(&ActorRef {
            phantom_marker: PhantomData::<M2>,
            sender: reply_tx,
        })));
        reply_rx
    }

    /// Best effort send a message to the associated actor
    pub fn tell(&self, message: M) {
        let _ = self.sender.try_send(Box::new(message));
    }
}

impl<M> Clone for ActorRef<M> {
    fn clone(&self) -> ActorRef<M> {
        ActorRef {
            phantom_marker: PhantomData,
            sender: self.sender.to_owned(),
        }
    }
}

impl<M> Debug for ActorRef<M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.sender.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use channel::{ReceiverImpl, SenderImpl};

    use super::*;

    use std::sync::mpsc::{sync_channel, Receiver as SyncReceiver, SyncSender};
    use std::sync::mpsc::{RecvError as SyncRecvError, TrySendError as SyncTrySendError};

    // This test provides reasonable coverage across the core APIs. A dispatcher is
    // set up to process command messages and actor messages synchronously to reason
    // the tests more easily.
    #[test]
    fn test_ask() {
        // Declare our actor to test - a simply recipient of a request and a reply

        struct Request {
            reply_to: ActorRef<Reply>,
            reply_with: u32,
        }
        struct Reply {
            value: u32,
        }
        struct MyActor {}
        impl Actor<Request> for MyActor {
            fn receive(&mut self, _context: &mut ActorContext<Request>, message: &Request) {
                message.reply_to.tell(Reply {
                    value: message.reply_with,
                })
            }
        }

        // Declares a dispatcher to process commands and actor messages synchronously

        struct MyDispatcher {
            command_tx: SyncSender<AnyMessage>,
        }
        impl MyDispatcher {
            pub fn receive_command(
                &self,
                command_rx: &SyncReceiver<AnyMessage>,
            ) -> Result<Option<SelectWithAction>, SyncRecvError> {
                match command_rx.recv() {
                    Ok(message) => Ok(match message.downcast::<DispatcherCommand>() {
                        Ok(dispatcher_command) => match *dispatcher_command {
                            DispatcherCommand::SelectWithAction { underlying } => Some(underlying),
                            DispatcherCommand::Stop => None,
                        },
                        Err(e) => {
                            warn!(
                                "Error received when expecting a dispatcher command: {:?}",
                                e
                            );
                            None
                        }
                    }),
                    Err(e) => Err(e),
                }
            }
            pub fn receive_messages(&self, selection: &mut SelectWithAction) {
                let receiver = selection
                    .receiver
                    .receiver_impl
                    .as_any()
                    .downcast_ref::<SyncReceiver<AnyMessage>>()
                    .unwrap();
                while let Ok(m) = receiver.try_recv() {
                    let active = (selection.action)(m);
                    if !active {
                        break;
                    }
                }
            }
        }
        impl Dispatcher for MyDispatcher {
            fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>> {
                self.command_tx
                    .try_send(Box::new(command))
                    .map_err(|e| match e {
                        SyncTrySendError::Disconnected(e) => TrySendError::Disconnected(e),
                        SyncTrySendError::Full(e) => TrySendError::Full(e),
                    })
            }
        }
        struct SyncReceiverImpl {
            receiver: SyncReceiver<AnyMessage>,
        }
        impl ReceiverImpl for SyncReceiverImpl {
            type Item = AnyMessage;

            fn as_any(&mut self) -> &mut (dyn Any + Send) {
                &mut self.receiver
            }
        }
        struct SyncSenderImpl {
            sender: SyncSender<AnyMessage>,
        }
        impl SenderImpl for SyncSenderImpl {
            type Item = AnyMessage;

            fn clone(&self) -> Box<dyn SenderImpl<Item = AnyMessage> + Send + Sync> {
                Box::new(SyncSenderImpl {
                    sender: self.sender.to_owned(),
                })
            }

            fn try_send(&self, msg: AnyMessage) -> Result<(), TrySendError<AnyMessage>> {
                match self.sender.try_send(msg) {
                    Ok(_) => Ok(()),
                    Err(SyncTrySendError::Disconnected(e)) => Err(TrySendError::Disconnected(e)),
                    Err(SyncTrySendError::Full(e)) => Err(TrySendError::Full(e)),
                }
            }
        }
        pub fn sync_mailbox_fn(
            buffer: usize,
        ) -> Box<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync> {
            Box::new(move || {
                let (mailbox_tx, mailbox_rx) = sync_channel(buffer);
                (
                    Sender {
                        sender_impl: Box::new(SyncSenderImpl {
                            sender: mailbox_tx.to_owned(),
                        }),
                    },
                    Receiver {
                        receiver_impl: Box::new(SyncReceiverImpl {
                            receiver: mailbox_rx,
                        }),
                    },
                )
            })
        }

        // Establish the dispatcher and mailbox

        let (command_tx, command_rx) = sync_channel::<AnyMessage>(1);
        let dispatcher = Arc::new(MyDispatcher { command_tx });

        let mailbox_fn = Arc::new(sync_mailbox_fn(1));

        // Establish our root actor and get a handle to the actor's receiver

        let my_actor = ActorContext::<Request>::new(
            || Box::new(MyActor {}),
            dispatcher.to_owned(),
            mailbox_fn.to_owned(),
        );
        let selection = dispatcher.receive_command(&command_rx);

        // Send an ask request to the actor and have the actor process its
        // messages

        let expected_reply_value = 10;

        let mut ask_receiver = my_actor.actor_ref.ask(
            &|reply_to| Request {
                reply_to: reply_to.to_owned(),
                reply_with: expected_reply_value,
            },
            mailbox_fn,
        );
        dispatcher.receive_messages(&mut selection.unwrap().unwrap());

        // Test the ask reply

        assert_eq!(
            ask_receiver
                .receiver_impl
                .as_any()
                .downcast_ref::<SyncReceiver<AnyMessage>>()
                .unwrap()
                .recv()
                .unwrap()
                .downcast_ref::<Reply>()
                .unwrap()
                .value,
            expected_reply_value
        )
    }
}

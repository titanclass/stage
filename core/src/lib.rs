#![cfg_attr(not(test), no_std)]

use core::{any::Any, fmt::Debug, marker::PhantomData, time::Duration};

extern crate alloc;
use crate::alloc::borrow::ToOwned;
use crate::alloc::{boxed::Box, sync::Arc};

pub mod channel;
use channel::{Receiver, RecvError, RecvTimeoutError, Sender, TrySendError};

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

    /// Tells the dispatcher to finish up. The thread on which the select
    /// function is running can then be joined.
    Stop,
}

/// A dispatcher composes a executor to call upon the actor's message queue, ultimately calling
/// upon the actor's receive method.
pub trait Dispatcher {
    /// Select all receivers and dispatch their actions. On dispatching on action, their
    /// selection should become ineligible so that they cannot be selected on another message until
    /// they have completed their processing. Once complete, the action should be followed by
    /// an enqueuing of their selection once more by calling upon the send function.
    fn select(&self) -> Result<AnyMessage, RecvError>;

    /// Enqueue a command to the channel being selected on.
    fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>>;

    /// Stop the current dispatcher and associated executor. This call is blocking and will
    /// return once all actors have stopped running.
    fn stop(&self);
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
                            Ok(boxed_m) => {
                                let m = *boxed_m;
                                actor.receive(&mut dispatcher_context, &m);
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
    pub async fn ask<F, M2>(&self, _f: F, _recv_timeout: Duration) -> Result<M2, RecvTimeoutError>
    where
        F: FnOnce() -> M + 'static,
    {
        unimplemented!() // FIXME
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
mod tests {} // FIXME: Need tests at this level!

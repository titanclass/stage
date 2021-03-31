use std::any::Any;

use crossbeam_channel::{
    bounded, unbounded, Receiver as CBReceiver, Select, Sender as CBSender,
    TryRecvError as CBTryRecvError, TrySendError as CBTrySendError,
};
use executors::*;
use executors::{crossbeam_workstealing_pool, parker::Parker};
use stage_core::channel::{Receiver, ReceiverImpl, RecvError, Sender, SenderImpl, TrySendError};

use log::{debug, warn};
use stage_core::{AnyMessage, Dispatcher, DispatcherCommand, SelectWithAction};

/// Provides an executor based on the executors package implementation of
/// crossbeam_workstealing_pool. In addition, the channels available for use
/// with mailbox and the command channel of a dispatcher are those of Crossbeam.

struct CBReceiverImpl {
    receiver: CBReceiver<AnyMessage>,
}

impl ReceiverImpl for CBReceiverImpl {
    type Item = AnyMessage;

    fn as_any(&self) -> &dyn Any {
        &self.receiver
    }
}

struct CBSenderImpl {
    sender: CBSender<AnyMessage>,
}

impl SenderImpl for CBSenderImpl {
    type Item = AnyMessage;

    fn clone(&self) -> Box<dyn SenderImpl<Item = AnyMessage>> {
        Box::new(CBSenderImpl {
            sender: self.sender.to_owned(),
        })
    }

    fn try_send(&self, msg: AnyMessage) -> Result<(), TrySendError<AnyMessage>> {
        match self.sender.try_send(msg) {
            Ok(_) => Ok(()),
            Err(CBTrySendError::Disconnected(e)) => Err(TrySendError::Disconnected(e)),
            Err(CBTrySendError::Full(e)) => Err(TrySendError::Full(e)),
        }
    }
}

/// Creates a Crossbeam-based bounded mailbox for communicating with an actor.
pub fn bounded_mailbox_fn(
    cap: usize,
) -> Box<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync> {
    Box::new(move || {
        let (mailbox_tx, mailbox_rx) = bounded::<AnyMessage>(cap);
        (
            Sender {
                sender_impl: Box::new(CBSenderImpl {
                    sender: mailbox_tx.to_owned(),
                }),
            },
            Receiver {
                receiver_impl: Box::new(CBReceiverImpl {
                    receiver: mailbox_rx,
                }),
            },
        )
    })
}

/// Creates a Crossbeam-based bounded mailbox for communicating with an actor.
pub fn unbounded_mailbox_fn(
) -> Box<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync> {
    Box::new(|| {
        let (mailbox_tx, mailbox_rx) = unbounded::<AnyMessage>();
        (
            Sender {
                sender_impl: Box::new(CBSenderImpl {
                    sender: mailbox_tx.to_owned(),
                }),
            },
            Receiver {
                receiver_impl: Box::new(CBReceiverImpl {
                    receiver: mailbox_rx,
                }),
            },
        )
    })
}

/// A Dispatcher for Stage that leverages the Executors crate's work-stealing ThreadPool.
///
/// The following code declares a dispatcher that work-steals across 4 cores. An unbounded
/// channel is established for internal communication with the dispatcher. Consideration
/// should be given to bounded channels upon its domain being understood and throughput
/// having been measured.
/// ```
/// use std::sync::Arc;
/// use crossbeam_channel::unbounded;
/// use executors::crossbeam_workstealing_pool;
/// use stage_dispatch_executors::WorkStealingPoolDispatcher;
///
/// let dispatcher_pool = crossbeam_workstealing_pool::small_pool(4);
/// let dispatcher = Arc::new(WorkStealingPoolDispatcher {
///     pool: dispatcher_pool,
///     command_channel: unbounded(),
/// });
/// ```

pub struct WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    pub pool: crossbeam_workstealing_pool::ThreadPool<P>,
    pub command_channel: (CBSender<AnyMessage>, CBReceiver<AnyMessage>),
}

impl<P> Dispatcher for WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    fn select(&self) -> Result<AnyMessage, RecvError> {
        let mut actionable_receivers: Vec<(CBReceiver<AnyMessage>, Box<SelectWithAction>)> = vec![];
        loop {
            let mut sel = Select::new();
            sel.recv(&self.command_channel.1); // The first one added is always our control channel for receiving commands
            actionable_receivers.iter().for_each(|command| {
                sel.recv(&command.0);
            });
            let oper = sel.select();

            let index = oper.index();
            let receiver = match index {
                0 => &self.command_channel.1,
                _ => &actionable_receivers[index - 1].0,
            };
            let res = oper.recv(receiver);

            if index > 0 {
                // Handle a message destined for an actor - this is the common case.
                let mut current_select_command = actionable_receivers.swap_remove(index - 1);
                match res {
                    Ok(message) => {
                        let tx = self.command_channel.0.to_owned();
                        self.pool.execute(move || {
                            let receiver = current_select_command.0;
                            let mut active = (current_select_command.1.action)(message);
                            while active {
                                match receiver.try_recv() {
                                    Ok(next_message) => {
                                        active = (current_select_command.1.action)(next_message)
                                    }
                                    Err(e) if e == CBTryRecvError::Empty => break,
                                    Err(e) => {
                                        debug!("Error received for actor {}", e);
                                        break;
                                    }
                                }
                            }
                            if active {
                                let _ = tx.send(current_select_command.1);
                            } else {
                                debug!("Actor has shutdown: {:?} - treating as a dead letter", tx);
                            }
                        });
                    }
                    Err(e) => {
                        debug!(
                            "Cannot receive on an actor channel: {} - treating as a dead letter",
                            e
                        );
                    }
                }
            } else {
                // Dispatcher message handling is prioritised to process SelectWithAction as we will
                // receive these ones mostly.
                match res {
                    Ok(message) => {
                        match message.downcast::<SelectWithAction>() {
                            Ok(select_with_action) => actionable_receivers.push((
                                select_with_action
                                    .receiver
                                    .receiver_impl
                                    .as_any()
                                    .downcast_ref::<CBReceiver<AnyMessage>>()
                                    .unwrap()
                                    .to_owned(),
                                select_with_action,
                            )),
                            Err(other_message_type) => {
                                match other_message_type.downcast::<DispatcherCommand>() {
                                    Ok(dispatcher_command) => match *dispatcher_command {
                                        DispatcherCommand::SelectWithAction { underlying } => {
                                            actionable_receivers.push((
                                                underlying
                                                    .receiver
                                                    .receiver_impl
                                                    .as_any()
                                                    .downcast_ref::<CBReceiver<AnyMessage>>()
                                                    .unwrap()
                                                    .to_owned(),
                                                Box::new(underlying),
                                            ));
                                        }
                                        DispatcherCommand::Stop => {
                                            self.pool.shutdown_async();
                                            return Ok(Box::new(DispatcherCommand::Stop));
                                        }
                                    },
                                    Err(e) => {
                                        warn!("Error received when expecting a dispatcher command: {:?}", e)
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {
                        return Err(RecvError);
                    }
                }
            }
        }
    }

    fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>> {
        self.command_channel
            .0
            .try_send(Box::new(command))
            .map_err(|e| match e {
                CBTrySendError::Disconnected(e) => TrySendError::Disconnected(e),
                CBTrySendError::Full(e) => TrySendError::Full(e),
            })
    }

    fn stop(&self) {
        let _ = self.send(DispatcherCommand::Stop);
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread, time::Duration};

    use crossbeam_channel::unbounded;

    use executors::crossbeam_workstealing_pool;
    use stage_core::{Actor, ActorContext, ActorRef};

    use super::*;

    fn init_logging() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_greeting() {
        init_logging();

        // Re-creates https://doc.akka.io/docs/akka/current/typed/actors.html#first-example

        // The messages

        struct Greet {
            whom: String,
            reply_to: ActorRef<Greeted>,
        }

        struct Greeted {
            whom: String,
            from: ActorRef<Greet>,
        }

        struct SayHello {
            name: String,
        }

        // The HelloWorld actor

        struct HelloWorld {}

        impl Actor<Greet> for HelloWorld {
            fn receive(&mut self, context: &mut ActorContext<Greet>, message: &Greet) {
                println!("Hello {}!", message.whom);
                message.reply_to.tell(Greeted {
                    whom: message.whom.to_owned(),
                    from: context.actor_ref.to_owned(),
                });
            }
        }

        // The HelloWorldBot actor

        struct HelloWorldBot {
            greeting_counter: u32,
            max: u32,
        }

        impl Actor<Greeted> for HelloWorldBot {
            fn receive(&mut self, context: &mut ActorContext<Greeted>, message: &Greeted) {
                let n = self.greeting_counter + 1;
                println!("Greeting {} for {}", n, message.whom);
                if n == self.max {
                    context.stop();
                } else {
                    message.from.tell(Greet {
                        whom: message.whom.to_owned(),
                        reply_to: context.actor_ref.to_owned(),
                    });
                    self.greeting_counter = n;
                }
            }
        }

        // The root actor

        struct HelloWorldMain {
            greeter: Option<ActorRef<Greet>>,
        }

        impl Actor<SayHello> for HelloWorldMain {
            fn receive(&mut self, context: &mut ActorContext<SayHello>, message: &SayHello) {
                let greeter = match &self.greeter {
                    None => {
                        let greeter = context.spawn(|| Box::new(HelloWorld {}));
                        self.greeter = Some(greeter.to_owned());
                        greeter
                    }
                    Some(greeter) => greeter.to_owned(),
                };

                let reply_to = context.spawn(|| {
                    Box::new(HelloWorldBot {
                        greeting_counter: 0,
                        max: 3,
                    })
                });
                greeter.tell(Greet {
                    whom: message.name.to_owned(),
                    reply_to,
                });
            }
        }

        // Establish our dispatcher.

        let dispatcher_pool = crossbeam_workstealing_pool::small_pool(4);
        let dispatcher = Arc::new(WorkStealingPoolDispatcher {
            pool: dispatcher_pool,
            command_channel: unbounded(),
        });

        // Establish a function to produce mailboxes

        let mailbox_fn = Arc::new(unbounded_mailbox_fn());

        // Create a root context, which is essentiallly the actor system. We
        // also send a couple of messages for our demo.

        let system = ActorContext::<SayHello>::new(
            || Box::new(HelloWorldMain { greeter: None }),
            dispatcher.to_owned(),
            mailbox_fn,
        );

        system.actor_ref.tell(SayHello {
            name: "World".to_string(),
        });

        system.actor_ref.tell(SayHello {
            name: "Stage".to_string(),
        });

        // Run the dispatcher select function on its own thread. We wait
        // for the select function to finish, which will be when will
        // tell the "system" (the actor context above) to stop, it is stops
        // itself.

        let select_thread_dispatcher = dispatcher.to_owned();
        let select_thread = thread::spawn(move || select_thread_dispatcher.select());

        thread::sleep(Duration::from_millis(500));

        dispatcher.stop();

        assert!(select_thread.join().is_ok());
    }
}

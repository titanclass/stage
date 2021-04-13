use std::{any::Any, marker::PhantomData, time::Duration};

use crossbeam_channel::{
    bounded, unbounded, Receiver as CbReceiver, RecvError as CbRecvError, RecvTimeoutError, Select,
    Sender as CbSender, TryRecvError as CbTryRecvError, TrySendError as CbTrySendError,
};
use executors::*;
use executors::{crossbeam_workstealing_pool, parker::Parker};
use stage_core::{
    channel::{Receiver, ReceiverImpl, Sender, SenderImpl, TrySendError},
    ActorRef,
};

use log::{debug, warn};
use stage_core::{AnyMessage, Dispatcher, DispatcherCommand, SelectWithAction};

/// Provides an executor based on the executors package implementation of
/// crossbeam_workstealing_pool. In addition, the channels available for use
/// with mailbox and the command channel of a dispatcher are those of Crossbeam.

struct CbReceiverImpl {
    receiver: CbReceiver<AnyMessage>,
}

impl ReceiverImpl for CbReceiverImpl {
    type Item = AnyMessage;

    fn as_any(&mut self) -> &mut (dyn Any + Send) {
        &mut self.receiver
    }
}

// A convenience for extracting a Crossbeam Receiver from a Stage Receiver type.
macro_rules! receiver {
    ($receiver:expr) => {
        $receiver
            .receiver_impl
            .as_any()
            .downcast_ref::<CbReceiver<AnyMessage>>()
            .unwrap()
    };
}

struct CbSenderImpl {
    sender: CbSender<AnyMessage>,
}

impl SenderImpl for CbSenderImpl {
    type Item = AnyMessage;

    fn clone(&self) -> Box<dyn SenderImpl<Item = AnyMessage> + Send + Sync> {
        Box::new(CbSenderImpl {
            sender: self.sender.to_owned(),
        })
    }

    fn try_send(&self, msg: AnyMessage) -> Result<(), TrySendError<AnyMessage>> {
        match self.sender.try_send(msg) {
            Ok(_) => Ok(()),
            Err(CbTrySendError::Disconnected(e)) => Err(TrySendError::Disconnected(e)),
            Err(CbTrySendError::Full(e)) => Err(TrySendError::Full(e)),
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
                sender_impl: Box::new(CbSenderImpl {
                    sender: mailbox_tx.to_owned(),
                }),
            },
            Receiver {
                receiver_impl: Box::new(CbReceiverImpl {
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
                sender_impl: Box::new(CbSenderImpl {
                    sender: mailbox_tx.to_owned(),
                }),
            },
            Receiver {
                receiver_impl: Box::new(CbReceiverImpl {
                    receiver: mailbox_rx,
                }),
            },
        )
    })
}

type AskResult<M2> = Result<Box<M2>, RecvTimeoutError>;

/// Implements the Ask pattern where a request message is sent
/// and a reply is expected.
pub trait Ask<M, M2> {
    /// Perform a one-shot ask operation while passing in a function
    /// to construct a message that accepts a reply_to sender.
    /// All asks have a timeout. Note also that if an unexpected
    /// reply is received then a timeout error will be indicated.
    /// Note that this method will block.
    fn ask(&self, request_fn: &dyn Fn(&ActorRef<M2>) -> M, recv_timeout: Duration)
        -> AskResult<M2>;
}

impl<M, M2> Ask<M, M2> for ActorRef<M>
where
    M: Send + 'static,
    M2: Send + 'static,
{
    fn ask(
        &self,
        request_fn: &dyn Fn(&ActorRef<M2>) -> M,
        recv_timeout: Duration,
    ) -> AskResult<M2> {
        let (reply_tx, reply_rx) = bounded::<AnyMessage>(1);
        let _ = self.sender.try_send(Box::new(request_fn(&ActorRef {
            phantom_marker: PhantomData::<M2>,
            sender: Sender {
                sender_impl: Box::new(CbSenderImpl { sender: reply_tx }),
            },
        })));
        reply_rx
            .recv_timeout(recv_timeout)
            .map(|message| {
                message
                    .downcast::<M2>()
                    .map_err(|_| RecvTimeoutError::Timeout)
            })
            .and_then(|v| v)
    }
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
/// use stage_dispatch_crossbeam_executors::WorkStealingPoolDispatcher;
///
/// let pool = crossbeam_workstealing_pool::small_pool(4);
/// let (command_tx, command_rx) = unbounded();
/// let dispatcher = Arc::new(WorkStealingPoolDispatcher { pool, command_tx });
/// // command_rx is then used later when using [start].
/// ```

pub struct WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    pub pool: crossbeam_workstealing_pool::ThreadPool<P>,
    pub command_tx: CbSender<AnyMessage>,
}

impl<P> WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    /// Start this dispatcher. Starting this dispatcher selects all receivers and dispatch their actions.
    /// On dispatching on action, their selection should becomes ineligible so that they cannot be selected
    /// on another message until they have completed their processing. Once complete, the action is be followed by
    /// an enqueuing of their selection once more by calling upon the send function.
    /// Note that this function is blocking.
    pub fn start(&self, command_rx: CbReceiver<AnyMessage>) -> Result<AnyMessage, CbRecvError> {
        let mut actionable_receivers: Vec<(CbReceiver<AnyMessage>, Box<SelectWithAction>)> = vec![];
        loop {
            let mut sel = Select::new();
            sel.recv(&command_rx); // The first one added is always our control channel for receiving commands
            actionable_receivers.iter().for_each(|command| {
                sel.recv(&command.0);
            });
            let oper = sel.select();

            let index = oper.index();
            let receiver = match index {
                0 => &command_rx,
                _ => &actionable_receivers[index - 1].0,
            };
            let res = oper.recv(receiver);

            if index > 0 {
                // Handle a message destined for an actor - this is the common case.
                let mut current_select_command = actionable_receivers.swap_remove(index - 1);
                match res {
                    Ok(message) => {
                        let tx = self.command_tx.to_owned();
                        self.pool.execute(move || {
                            let receiver = current_select_command.0;
                            let mut active = (current_select_command.1.action)(message);
                            while active {
                                match receiver.try_recv() {
                                    Ok(next_message) => {
                                        active = (current_select_command.1.action)(next_message)
                                    }
                                    Err(e) if e == CbTryRecvError::Empty => break,
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
                            Ok(mut select_with_action) => actionable_receivers.push((
                                receiver!(select_with_action.receiver).to_owned(),
                                select_with_action,
                            )),
                            Err(other_message_type) => {
                                match other_message_type.downcast::<DispatcherCommand>() {
                                    Ok(dispatcher_command) => match *dispatcher_command {
                                        DispatcherCommand::SelectWithAction { mut underlying } => {
                                            actionable_receivers.push((
                                                receiver!(underlying.receiver).to_owned(),
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
                    e @ Err(_) => {
                        return e;
                    }
                }
            }
        }
    }
}

impl<P> Dispatcher for WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>> {
        self.command_tx
            .try_send(Box::new(command))
            .map_err(|e| match e {
                CbTrySendError::Disconnected(e) => TrySendError::Disconnected(e),
                CbTrySendError::Full(e) => TrySendError::Full(e),
            })
    }
}

impl<P> Drop for WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    fn drop(&mut self) {
        self.stop()
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

        let pool = crossbeam_workstealing_pool::small_pool(4);
        let (command_tx, command_rx) = unbounded();
        let dispatcher = Arc::new(WorkStealingPoolDispatcher { pool, command_tx });

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

        // Run the dispatcher on its own thread. We wait for the run
        // function to finish, which will be when will we tell the "system"
        // (the actor context above) to stop, it is stops itself.

        let dispatcher_thread_dispatcher = dispatcher.to_owned();
        let dispatcher_thread =
            thread::spawn(move || dispatcher_thread_dispatcher.start(command_rx));

        thread::sleep(Duration::from_millis(500));

        dispatcher.stop();

        assert!(dispatcher_thread.join().is_ok());
    }
}

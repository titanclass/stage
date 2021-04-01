use std::any::Any;

use log::warn;
use stage_core::channel::{Receiver, ReceiverImpl, Sender, SenderImpl, TrySendError};
use tokio::sync::mpsc::{
    channel, unbounded_channel, Receiver as TkReceiver, Sender as TkSender,
    UnboundedReceiver as TkUnboundedReceiver, UnboundedSender as TkUnboundedSender,
};
use tokio::{
    sync::mpsc::error::{SendError as TkSendError, TrySendError as TkTrySendError},
    task::JoinHandle,
};

use stage_core::{AnyMessage, Dispatcher, DispatcherCommand};

/// Provides an Tokio based dispatcher.

struct TkReceiverImpl {
    receiver: TkReceiver<AnyMessage>,
}

impl ReceiverImpl for TkReceiverImpl {
    type Item = AnyMessage;

    fn as_any(&mut self) -> &mut (dyn Any + Send) {
        &mut self.receiver
    }
}

struct TkSenderImpl {
    sender: TkSender<AnyMessage>,
}

impl SenderImpl for TkSenderImpl {
    type Item = AnyMessage;

    fn clone(&self) -> Box<dyn SenderImpl<Item = AnyMessage> + Send + Sync> {
        Box::new(TkSenderImpl {
            sender: self.sender.to_owned(),
        })
    }

    fn try_send(&self, msg: AnyMessage) -> Result<(), TrySendError<AnyMessage>> {
        match self.sender.try_send(msg) {
            Ok(_) => Ok(()),
            Err(TkTrySendError::Closed(e)) => Err(TrySendError::Disconnected(e)),
            Err(TkTrySendError::Full(e)) => Err(TrySendError::Full(e)),
        }
    }
}

/// Creates a Tokio bounded mailbox for communicating with an actor.
pub fn mailbox_fn(
    buffer: usize,
) -> Box<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync> {
    Box::new(move || {
        let (mailbox_tx, mailbox_rx) = channel(buffer);
        (
            Sender {
                sender_impl: Box::new(TkSenderImpl {
                    sender: mailbox_tx.to_owned(),
                }),
            },
            Receiver {
                receiver_impl: Box::new(TkReceiverImpl {
                    receiver: mailbox_rx,
                }),
            },
        )
    })
}

struct TkUnboundedReceiverImpl {
    receiver: TkUnboundedReceiver<AnyMessage>,
}

impl ReceiverImpl for TkUnboundedReceiverImpl {
    type Item = AnyMessage;

    fn as_any(&mut self) -> &mut (dyn Any + Send) {
        &mut self.receiver
    }
}

struct TkUnboundedSenderImpl {
    sender: TkUnboundedSender<AnyMessage>,
}

impl SenderImpl for TkUnboundedSenderImpl {
    type Item = AnyMessage;

    fn clone(&self) -> Box<dyn SenderImpl<Item = AnyMessage> + Send + Sync> {
        Box::new(TkUnboundedSenderImpl {
            sender: self.sender.to_owned(),
        })
    }

    fn try_send(&self, msg: AnyMessage) -> Result<(), TrySendError<AnyMessage>> {
        match self.sender.send(msg) {
            Ok(_) => Ok(()),
            Err(TkSendError(e)) => Err(TrySendError::Disconnected(e)),
        }
    }
}

/// Creates a Tokio unbounded mailbox for communicating with an actor.
pub fn unbounded_mailbox_fn(
) -> Box<dyn Fn() -> (Sender<AnyMessage>, Receiver<AnyMessage>) + Send + Sync> {
    Box::new(|| {
        let (mailbox_tx, mailbox_rx) = unbounded_channel();
        (
            Sender {
                sender_impl: Box::new(TkUnboundedSenderImpl {
                    sender: mailbox_tx.to_owned(),
                }),
            },
            Receiver {
                receiver_impl: Box::new(TkUnboundedReceiverImpl {
                    receiver: mailbox_rx,
                }),
            },
        )
    })
}
struct TaskStopped {
    id: usize,
}

// A macro describing the dispatching logic for both bounded and unbounded receivers
macro_rules! start {
    ($receiver_type:ty, $command_tx:expr, $command_rx:expr) => {{
        let mut spawned_tasks: Vec<(usize, JoinHandle<()>)> = vec![];
        while let Some(message) = $command_rx.recv().await {
            match message.downcast::<DispatcherCommand>() {
                Ok(dispatcher_command) => match *dispatcher_command {
                    DispatcherCommand::SelectWithAction { mut underlying } => {
                        let id = spawned_tasks.len();
                        let tx = $command_tx.to_owned();
                        let join_handle = tokio::spawn(async move {
                            let receiver = underlying
                                .receiver
                                .receiver_impl
                                .as_any()
                                .downcast_mut::<$receiver_type>()
                                .unwrap();
                            while let Some(m) = receiver.recv().await {
                                let active = (underlying.action)(m);
                                if !active {
                                    break;
                                }
                            }
                            let _ = tx.send(Box::new(TaskStopped { id }));
                        });
                        spawned_tasks.push((id, join_handle));
                    }
                    DispatcherCommand::Stop => {
                        break;
                    }
                },
                Err(other_message_type) => match other_message_type.downcast::<TaskStopped>() {
                    Ok(task_stopped) => {
                        let _ = spawned_tasks.swap_remove(task_stopped.id);
                    }
                    Err(e) => {
                        warn!(
                            "Error received when expecting a dispatcher command: {:?}",
                            e
                        )
                    }
                },
            }
        }
    }};
}

/// A Dispatcher for Stage that leverages the Tokio runtime and a bounded command channel.

pub struct TokioDispatcher {
    pub command_tx: TkSender<AnyMessage>,
}

impl TokioDispatcher {
    pub async fn start(&self, mut command_rx: TkReceiver<AnyMessage>) {
        start!(TkReceiver<AnyMessage>, self.command_tx, command_rx)
    }
}

impl Dispatcher for TokioDispatcher {
    fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>> {
        self.command_tx
            .try_send(Box::new(command))
            .map_err(|e| match e {
                TkTrySendError::Closed(e) => TrySendError::Disconnected(e),
                TkTrySendError::Full(e) => TrySendError::Full(e),
            })
    }
}

impl Drop for TokioDispatcher {
    fn drop(&mut self) {
        self.stop()
    }
}

/// A Dispatcher for Stage that leverages the Tokio runtime and an unbounded command channel.

pub struct TokioUnboundedDispatcher {
    pub command_tx: TkUnboundedSender<AnyMessage>,
}

impl TokioUnboundedDispatcher {
    pub async fn start(&self, mut command_rx: TkUnboundedReceiver<AnyMessage>) {
        start!(TkUnboundedReceiver<AnyMessage>, self.command_tx, command_rx)
    }
}

impl Drop for TokioUnboundedDispatcher {
    fn drop(&mut self) {
        self.stop()
    }
}

impl Dispatcher for TokioUnboundedDispatcher {
    fn send(&self, command: DispatcherCommand) -> Result<(), TrySendError<AnyMessage>> {
        self.command_tx
            .send(Box::new(command))
            .map_err(|e| match e {
                TkSendError(e) => TrySendError::Disconnected(e),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tokio::{sync::mpsc::unbounded_channel, time::sleep};

    use stage_core::{Actor, ActorContext, ActorRef};

    use super::*;

    fn init_logging() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_greeting() {
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

        let (command_tx, command_rx) = unbounded_channel();
        let dispatcher = Arc::new(TokioUnboundedDispatcher { command_tx });

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

        let dispatcher_task_dispatcher = dispatcher.to_owned();
        let dispatcher_task =
            tokio::spawn(async move { dispatcher_task_dispatcher.start(command_rx).await });

        let _ = sleep(Duration::from_millis(500)).await;

        dispatcher.stop();

        assert!(dispatcher_task.await.is_ok())
    }
}

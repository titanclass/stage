use std::any::Any;

use crossbeam_channel::{Receiver, RecvError, Select, SendError, Sender};
use executors::*;
use executors::{crossbeam_workstealing_pool, parker::Parker};

use log::{debug, warn};
use stage_core::{Dispatcher, DispatcherCommand, SelectWithAction};

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
/// let (dispatcher_tx, dispatcher_rx) = unbounded();
/// let dispatcher = Arc::new(WorkStealingPoolDispatcher {
///     pool: dispatcher_pool,
///     rx: dispatcher_rx,
///     tx: dispatcher_tx,
/// });
/// ```

pub struct WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    pub pool: crossbeam_workstealing_pool::ThreadPool<P>,
    pub rx: Receiver<Box<dyn Any + Send>>,
    pub tx: Sender<Box<dyn Any + Send>>,
}

impl<P> Dispatcher for WorkStealingPoolDispatcher<P>
where
    P: Parker + Clone + 'static,
{
    fn select(&self) -> Result<Box<dyn Any + Send>, RecvError> {
        let mut select_commands: Vec<Box<SelectWithAction>> = vec![];
        loop {
            let mut sel = Select::new();
            sel.recv(&self.rx); // The first one added is always our control channel for receiving commands
            select_commands.iter().for_each(|command| {
                sel.recv(&command.receiver);
            });
            let oper = sel.select();

            let index = oper.index();
            let receiver = match index {
                0 => &self.rx,
                _ => &select_commands[index - 1].receiver,
            };
            let res = oper.recv(receiver);

            if index > 0 {
                // Handle a message destined for an actor - this is the common case.
                let mut current_select_command = select_commands.swap_remove(index - 1);
                match res {
                    Ok(message) => {
                        let tx = self.tx.to_owned();
                        self.pool.execute(move || {
                            if (current_select_command.action)(message) {
                                let _ = tx.send(current_select_command);
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
                            Ok(select_with_action) => select_commands.push(select_with_action),
                            Err(other_message_type) => {
                                match other_message_type.downcast::<DispatcherCommand>() {
                                    Ok(dispatcher_command) => match *dispatcher_command {
                                        DispatcherCommand::SelectWithAction { underlying } => {
                                            select_commands.push(Box::new(underlying));
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
                        return res;
                    }
                }
            }
        }
    }

    fn send(&self, command: DispatcherCommand) -> Result<(), SendError<Box<dyn Any + Send>>> {
        self.tx.send(Box::new(command))
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
                        let greeter = context.spawn(|| Box::new(HelloWorld {}), "greeter");
                        self.greeter = Some(greeter.to_owned());
                        greeter
                    }
                    Some(greeter) => greeter.to_owned(),
                };

                let reply_to = context.spawn(
                    || {
                        Box::new(HelloWorldBot {
                            greeting_counter: 0,
                            max: 3,
                        })
                    },
                    &message.name,
                );
                greeter.tell(Greet {
                    whom: message.name.to_owned(),
                    reply_to,
                });
            }
        }

        // Establish our dispatcher.

        let dispatcher_pool = crossbeam_workstealing_pool::small_pool(4);
        let (dispatcher_tx, dispatcher_rx) = unbounded();
        let dispatcher = Arc::new(WorkStealingPoolDispatcher {
            pool: dispatcher_pool,
            rx: dispatcher_rx,
            tx: dispatcher_tx,
        });

        // Create a root context, which is essentiallly the actor system. We
        // also send a couple of messages for our demo.

        let system = ActorContext::<SayHello>::new(
            || Box::new(HelloWorldMain { greeter: None }),
            "hello",
            dispatcher.to_owned(),
            Arc::new(|| unbounded()),
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

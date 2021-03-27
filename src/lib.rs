use std::{any::Any, fmt::Display, marker::PhantomData, sync::Arc, time::Duration};

use crossbeam_channel::{Receiver, RecvError, RecvTimeoutError, SendError, Sender, TryRecvError};

use log::{debug, warn};

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
    pub receiver: Receiver<Box<dyn Any + Send>>,
    pub action: Box<dyn FnMut(Box<dyn Any + Send>) -> bool + Send + Sync>,
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
    fn select(&self) -> Result<Box<dyn Any + Send>, RecvError>;

    /// Enqueue a command to the channel being selected on.
    fn send(&self, command: DispatcherCommand) -> Result<(), SendError<Box<dyn Any + Send>>>;

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
    pub mailbox_fn:
        Arc<dyn Fn() -> (Sender<Box<dyn Any + Send>>, Receiver<Box<dyn Any + Send>>) + Send + Sync>,
    pub name: Arc<String>,
}

impl<M> ActorContext<M> {
    /// Create a new actor context and associate it with a dispatcher.
    pub fn new<FA>(
        new_actor_fn: FA,
        name: &str,
        dispatcher: Arc<dyn Dispatcher + Send + Sync>,
        mailbox_fn: Arc<
            dyn Fn() -> (Sender<Box<dyn Any + Send>>, Receiver<Box<dyn Any + Send>>) + Send + Sync,
        >,
    ) -> ActorContext<M>
    where
        FA: FnOnce() -> Box<dyn Actor<M> + Send + Sync>,
        M: Send + Sync + 'static,
    {
        let shared_name = Arc::new(name.to_owned());
        let (tx, rx) = mailbox_fn();
        let actor_ref = ActorRef {
            name: shared_name.to_owned(),
            phantom_marker: PhantomData,
            sender: tx,
        };
        let context = ActorContext {
            active: true,
            actor_ref: actor_ref.to_owned(),
            dispatcher: dispatcher.to_owned(),
            mailbox_fn,
            name: shared_name,
        };
        let mut actor = new_actor_fn();

        let mut dispatcher_context = context.to_owned();

        match dispatcher.send(DispatcherCommand::SelectWithAction {
            underlying: SelectWithAction {
                receiver: rx.to_owned(),
                action: Box::new(move |message| {
                    let mut next_message = message;
                    loop {
                        if dispatcher_context.active {
                            match next_message.downcast::<M>() {
                                Ok(boxed_m) => {
                                    let m = *boxed_m;
                                    actor.receive(&mut dispatcher_context, &m);
                                }
                                Err(m) => warn!(
                                    "Unexpected message in {}: type_id: {:?}",
                                    dispatcher_context.actor_ref,
                                    m.type_id()
                                ),
                            }
                        }
                        match rx.try_recv() {
                            Ok(m) => next_message = m,
                            Err(e) if e == TryRecvError::Empty => break,
                            Err(e) => {
                                debug!("Error received in {}: {}", dispatcher_context.actor_ref, e);
                                break;
                            }
                        }
                    }
                    dispatcher_context.active
                }),
            },
        }) {
            Err(e) => {
                debug!("Error received establishing {}: {}", actor_ref, e);
                ()
            }
            _ => (),
        }

        context
    }

    /// Create a new actor as a child to this one. The child actor will receive
    /// the same dispatcher as the current one.
    pub fn spawn<FA, M2>(&mut self, new_actor_fn: FA, name: &str) -> ActorRef<M2>
    where
        FA: FnOnce() -> Box<dyn Actor<M2> + Send + Sync>,
        M2: Send + Sync + 'static,
    {
        let context = ActorContext::<M2>::new(
            new_actor_fn,
            name,
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
            name: self.name.to_owned(),
        }
    }
}

/// An actor ref provides a means by which to communicate
/// with an actor; in fact it is the only means to send
/// a message to an actor. Any associated actor may no
/// longer exist, in which case messages will be delivered
/// to a dead letter channel.
pub struct ActorRef<M> {
    name: Arc<String>,
    phantom_marker: PhantomData<M>,
    sender: Sender<Box<dyn Any + Send>>,
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
        let _ = self.sender.send(Box::new(message));
    }
}

impl<M> Clone for ActorRef<M> {
    fn clone(&self) -> ActorRef<M> {
        ActorRef {
            name: self.name.to_owned(),
            phantom_marker: PhantomData,
            sender: self.sender.to_owned(),
        }
    }
}

impl<M> Display for ActorRef<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActorRef({})", self.name)
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use crossbeam_channel::{unbounded, Select};

    use executors::crossbeam_workstealing_pool;
    use executors::*;

    use super::*;

    fn init_logging() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_greeting() {
        init_logging();

        // Re-creates https://doc.akka.io/docs/akka/current/typed/actors.html#first-example

        // We must first creator an executor along with a dispatcher so our actors can schedule
        // their execution.
        // NOTE - virtually all of this dispatcher implementation will became available as part of
        // the library. It is laid out here so I can begin to understand its API so that we can
        // run with any executor library.

        struct PoolDispatcher {
            pool: crossbeam_workstealing_pool::ThreadPool<
                parker::StaticParker<parker::SmallThreadData>,
            >,
            rx: Receiver<Box<dyn Any + Send>>,
            tx: Sender<Box<dyn Any + Send>>,
        }

        impl Dispatcher for PoolDispatcher {
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
                                        debug!(
                                            "Actor has shutdown: {:?} - treating as a dead letter",
                                            tx
                                        );
                                    }
                                });
                            }
                            Err(e) => {
                                debug!("Cannot receive on an actor channel: {} - treating as a dead letter", e);
                            }
                        }
                    } else {
                        // Dispatcher message handling is prioritised to process SelectWithAction as we will
                        // receive these ones mostly.
                        match res {
                            Ok(message) => match message.downcast::<SelectWithAction>() {
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
                            },
                            Err(_) => {
                                return res;
                            }
                        }
                    }
                }
            }

            fn send(
                &self,
                command: DispatcherCommand,
            ) -> Result<(), SendError<Box<dyn Any + Send>>> {
                self.tx.send(Box::new(command))
            }

            fn stop(&self) {
                let _ = self.send(DispatcherCommand::Stop);
            }
        }

        let dispatcher_pool = crossbeam_workstealing_pool::small_pool(4);
        let (dispatcher_tx, dispatcher_rx) = unbounded();
        let dispatcher = Arc::new(PoolDispatcher {
            pool: dispatcher_pool,
            rx: dispatcher_rx,
            tx: dispatcher_tx,
        });

        // From here on in, pretty much regular user code.

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

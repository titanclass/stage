use std::{
    sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender},
    time::Duration,
};

pub trait Actor<M> {
    fn receive(&mut self, context: &mut ActorContext<M>, message: M);
}

pub struct ActorContext<M> {
    pub actor_ref: ActorRef<M>,
    actor: Box<dyn Actor<M>>,
    receiver: Receiver<M>,
}

impl<M> ActorContext<M> {
    pub fn new(actor: &dyn Fn() -> Box<dyn Actor<M>>, name: &str) -> ActorContext<M> {
        let (tx, rx) = channel::<M>();
        let actor_ref = ActorRef { sender: tx };
        ActorContext {
            actor_ref: actor_ref,
            actor: actor(),
            receiver: rx,
        }
    }

    pub fn spawn<M2>(
        &mut self,
        actor: &dyn Fn() -> Box<dyn Actor<M2>>,
        name: &str,
    ) -> ActorRef<M2> {
        let (tx, rx) = channel::<M2>();
        let actor_ref = ActorRef { sender: tx };
        let context = ActorContext {
            actor_ref: actor_ref.to_owned(),
            actor: actor(),
            receiver: rx,
        }; // FIXME store the context so we can refer to it
        actor_ref
    }

    pub fn stop(&mut self) {
        unimplemented!();
    }
}

/// An actor ref provides a means by which to communicate
/// with an actor; in fact it is the only means to send
/// a message to an actor. Any associated actor may no
/// longer exist, in which case messages will be delivered
/// to a dead letter channel.
pub struct ActorRef<M> {
    sender: Sender<M>,
}

impl<M> ActorRef<M> {
    /// Perform an ask operation on the associated actor
    /// while passing in a function to construct a message
    /// that accepts a reply_to sender
    pub async fn ask<M2>(
        &self,
        f: &dyn Fn(Sender<M>) -> M,
        recv_timeout: Duration,
    ) -> Result<M2, RecvTimeoutError> {
        unimplemented!()
    }

    /// Best effort send a message to the associated actor
    pub fn tell(&self, message: M) {
        let _ = self.sender.send(message);
    }
}

impl<M> Clone for ActorRef<M> {
    fn clone(&self) -> ActorRef<M> {
        ActorRef {
            sender: self.sender.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc::channel, thread};

    use super::*;

    #[test]
    fn test_greeting() {
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
            fn receive(&mut self, context: &mut ActorContext<Greet>, message: Greet) {
                println!("Hello {}!", message.whom);
                message.reply_to.tell(Greeted {
                    whom: message.whom,
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
            fn receive(&mut self, context: &mut ActorContext<Greeted>, message: Greeted) {
                let n = self.greeting_counter + 1;
                println!("Greeting {} for {}", n, message.whom);
                if n == self.max {
                    context.stop();
                } else {
                    message.from.tell(Greet {
                        whom: message.whom,
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
            fn receive(&mut self, context: &mut ActorContext<SayHello>, message: SayHello) {
                let greeter = match &self.greeter {
                    None => {
                        let greeter = context.spawn(&|| Box::new(HelloWorld {}), "greeter");
                        self.greeter = Some(greeter.to_owned());
                        greeter
                    }
                    Some(greeter) => greeter.to_owned(),
                };

                let reply_to = context.spawn(
                    &|| {
                        Box::new(HelloWorldBot {
                            greeting_counter: 0,
                            max: 3,
                        })
                    },
                    &message.name,
                );
                greeter.tell(Greet {
                    whom: message.name,
                    reply_to,
                });
            }
        }

        let system =
            ActorContext::<SayHello>::new(&|| Box::new(HelloWorldMain { greeter: None }), "hello");

        system.actor_ref.tell(SayHello {
            name: "World".to_string(),
        });

        system.actor_ref.tell(SayHello {
            name: "Stage".to_string(),
        });

        // // Create a channel for communication
        // let (tx, rx) = channel::<(u64, Greet)>();

        // // Create the actor
        // let hello_world = HelloWorld {
        //     context: ActorContext { actor_ref: ActorRef { id: 1, sender: tx } },
        // };

        // // Start the dispatch for the actor
        // thread::spawn(move || {
        //     while let Ok((id, message)) = rx.recv() {
        //         match id {
        //             1 => hello_world.receive(message),
        //             _ => (),
        //         };
        //     }
        // });

        // // Construct a reference to the actor
        // let root = ActorRef { id: 1, sender: tx };

        // // Send a message to the actor
        // root.tell(Greet { whom: "someone".to_string());
    }
}

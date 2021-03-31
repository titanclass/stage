\* \* \* EXPERIMENTAL \* \* \*

# Stage
[![Test](https://github.com/titanclass/stage/actions/workflows/test.yml/badge.svg)](https://github.com/titanclass/stage/actions/workflows/test.yml)

A minimal [actor model](https://en.wikipedia.org/wiki/Actor_model) library targeting `nostd` [Rust](https://www.rust-lang.org/) 
and designed to run with any executor.

# Why are actors useful?

Actors provide a programming convenience for concurrent computations. Actors can only receive messages, send more messages, 
and create more actors. Actors are guaranteed to only ever receive one message at a time and can maintain their own state
without the concern of locks. Given actor references, location transparency is also attainable where the sender of a message
has no knowledge of where the actor's execution takes place (current thread, another thread, another core, another machine...).

## Why Stage?

Stage's core library `stage_core` provides a minimal set of types and traits required
to sufficiently an express an actor model, and no more.

## Inspiration

We wish to acknowledge the [Akka](https://akka.io/) project as a great source of inspiration, with the authors of Stage also 
having applied Akka over the years. Akka's goal is to, "Build powerful reactive, concurrent, and distributed applications more easily".
We hope that Stage can be composed with other projects to achieve the same goals while using Rust.

## In a nutshell

The essential types are `Actor`, `ActorRef` and `ActorContext`. Actors need to run on something, and they are dispatched
to whatever that is via a `Dispatcher`. Other crates such as `stage_dispatch_crossbeam_executors` are available to provide an
execution environment.

Actor declarations look like this:

```rust
struct Greet {
    whom: String,
}

struct HelloWorld {}

impl Actor<Greet> for HelloWorld {
    fn receive(&mut self, context: &mut ActorContext<Greet>, message: &Greet) {
        println!("Hello {}!", message.whom);
    }
}
```

Dispatcher and mailbox setup looks like this - we will use the `stage_dispatch_crossbeam_executors` work-stealing pool for 4 processors 
along with an unbounded channel for communicating with it and for each actor:

```rust
let dispatcher_pool = crossbeam_workstealing_pool::small_pool(4);
let dispatcher = Arc::new(WorkStealingPoolDispatcher {
    pool: dispatcher_pool,
    command_channel: unbounded(),
});

let mailbox_fn = Arc::new(unbounded_mailbox_fn());
```

Communicating with the actor looks like this:

```rust
let system = ActorContext::<Greet>::new(
    || Box::new(HelloWorld {}),
    dispatcher,
    mailbox_fn,
);

system.actor_ref.tell(SayHello {
    name: "Stage".to_string(),
});
```

For complete examples, please consult the tests associated with each dispatcher library.

## What about...

### What about actor supervision?

Actor model libraries often include supervisory functions, although this is not a requirement
of the actor model per se.

We believe that supervisory concerns should be external to Stage. However, we may need
to provide the ability to watch actors so that supervisor implementations can be 
achieved. That said, we have found that trying to recover from unforeseen events may
point to a design concern. In many cases, panicking and having the process die (and then
restart given OS level supervision), or restarting an embedded device, will be a better
course of action.

### What about actor naming?

Many libraries permit their actors to be named. We see the naming of actors as an external
concern e.g. maintaining a hash map of names to actor refs.

### What about distributing actors across a network?

Stage does not concern itself directly with networking. A custom dispatcher should be able
to be written that dispatches work across a network.

## Contribution policy

Contributions via GitHub pull requests are gladly accepted from their original author. Along with any pull requests, please state that the contribution is your original work and that you license the work to the project under the project's open source license. Whether or not you state this explicitly, by submitting any copyrighted material via pull request, email, or other means you agree to license the material under the project's open source license and warrant that you have the legal authority to do so.

## License

This code is open source software licensed under the [Apache-2.0 license](./LICENSE).

© Copyright Titan Class P/L, 2020

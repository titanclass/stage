\* \* \* EXPERIMENTAL \* \* \*

# Stage
[![Test](https://github.com/titanclass/stage/actions/workflows/test.yml/badge.svg)](https://github.com/titanclass/stage/actions/workflows/test.yml)

A minimal [actor model](https://en.wikipedia.org/wiki/Actor_model) library targeting `nostd` [Rust](https://www.rust-lang.org/) 
and designed to run with any executor.

## Why are actors useful?

Actors provide a stateful programming convenience for concurrent computations. Actors can only receive messages, send more messages, 
and create more actors. Actors are guaranteed to only ever receive one message at a time and can maintain their own state
without the concern of locks. Given actor references, location transparency is also attainable where the sender of a message
has no knowledge of where the actor's execution takes place (current thread, another thread, another core, another machine...).

Actors are particularly good at hosting [Finite State Machines](https://en.wikipedia.org/wiki/Finite-state_machine), particularly
[event-driven ones](http://christopherhunt-software.blogspot.com/2021/02/event-driven-finite-state-machines.html).

## Why Stage?

Stage's core library `stage_core` provides a minimal set of types and traits required
to sufficiently express an actor model, and no more. The resulting actors should then
be able to run on [the popular async runtimes](https://rust-lang.github.io/async-book/08_ecosystem/00_chapter.html#popular-async-runtimes) available, including tokio and async-std.

## Inspiration

We wish to acknowledge the [Akka](https://akka.io/) project as a great source of inspiration, with the authors of Stage 
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
let pool = crossbeam_workstealing_pool::small_pool(4);
let (command_tx, command_rx) = unbounded();
let dispatcher = Arc::new(WorkStealingPoolDispatcher { pool, command_tx });

let mailbox_fn = Arc::new(unbounded_mailbox_fn());
```

Sending a message to an actor looks like this:

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

We are also able to perform request/reply scenarios using `ask`. For example, using tokio as a runtime:

```rust
actor_ref.ask(
    &|reply_to| Request {
        reply_to: reply_to.to_owned(),
    },
    Duration::from_secs(1),
)
```

The `ask` method of an actor reference takes a function that is responsible for producing a request 
with a `reply_to` actor reference. Asks always take a timeout parameter which, in the case of Tokio,
may return an `Elapsed` error.

For complete examples, please consult the tests associated with each dispatcher library.

## What about...

### Channels

Actors here build on channels and associate state with the receiver. The type system is used
to enforce actor semantics; in particular, requiring a single receiver so that an actor's
state can be mutated without contention.

### async/await within an actor

Using async/await (Futures) within an actor's `receive` method would permit calling out to async
functions of other libraries. However, a danger here is that these async functions may block 
indefinitely as there is no contractual obligation to ever return (an issue for discussing the
contractual obligations of async functions has been 
[raised on the Rust internals forum](https://internals.rust-lang.org/t/future-and-its-assurance-of-completion/14542)).
Blocking would prevent an actor from processing other messages in its mailbox.

Another argument here is that actors can be considered orthoganal to async/await. Actors make 
great state machines, and receiving commands, including ones to stop the state machine, should not
be blocked from processing.

Finally, async functions can call into actors by using the `ActorRef.ask` async method call. 
Therefore, async functions and actors are able to co-exist and potentially serve distinct use-cases.

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

© Copyright [Titan Class P/L](https://www.titanclass.com.au/), 2020

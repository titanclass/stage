use std::{sync::Arc, thread};

use criterion::{criterion_group, criterion_main, Criterion};
use crossbeam_channel::{bounded as cb_bounded, unbounded as cb_unbounded, Receiver as CbReceiver};
use executors::crossbeam_workstealing_pool;
use stage_core::{Actor, ActorContext, ActorRef, AnyMessage};
use stage_dispatch_crossbeam_executors::{
    bounded_mailbox_fn as cbe_bounded_mailbox_fn, unbounded_mailbox_fn as cbe_unbounded_mailbox_fn,
    WorkStealingPoolDispatcher,
};
use stage_dispatch_tokio::{
    mailbox_fn as tk_mailbox_fn, unbounded_mailbox_fn as tk_unbounded_mailbox_fn, TokioDispatcher,
    TokioUnboundedDispatcher,
};
use tokio::sync::mpsc::{
    channel as tk_channel, unbounded_channel as tk_unbounded_channel, Receiver as TkReceiver,
    UnboundedReceiver as TkUnboundedReceiver,
};

// Fixtures

struct Request {
    reply_to: ActorRef<Reply>,
}
struct Reply {}
struct MyActor {}
impl Actor<Request> for MyActor {
    fn receive(&mut self, _context: &mut ActorContext<Request>, message: &Request) {
        message.reply_to.tell(Reply {})
    }
}

// Benchmarks

fn send_messages_to_other_actors_bounded_cbe(c: &mut Criterion) {
    let pool = crossbeam_workstealing_pool::small_pool(4);
    let (command_tx, command_rx) = cb_bounded(1);
    let dispatcher = Arc::new(WorkStealingPoolDispatcher { pool, command_tx });

    let mailbox_fn = Arc::new(cbe_bounded_mailbox_fn(1));

    let dispatcher_thread_dispatcher = dispatcher.to_owned();
    let _ = thread::spawn(move || dispatcher_thread_dispatcher.start(command_rx));

    let my_actor = ActorContext::<Request>::new(
        || Box::new(MyActor {}),
        dispatcher.to_owned(),
        mailbox_fn.to_owned(),
    );

    c.bench_function(
        "send messages to other actors using Crossbeam and Executors with bounded channels",
        |b| {
            b.iter(|| {
                let mut ask_receiver = my_actor.actor_ref.ask(
                    &|reply_to| Request {
                        reply_to: reply_to.to_owned(),
                    },
                    mailbox_fn.to_owned(),
                );
                let _ = ask_receiver
                    .receiver_impl
                    .as_any()
                    .downcast_ref::<CbReceiver<AnyMessage>>()
                    .unwrap()
                    .recv()
                    .unwrap()
                    .downcast_ref::<Reply>()
                    .unwrap();
            });
        },
    );
}

fn send_messages_to_other_actors_unbounded_cbe(c: &mut Criterion) {
    let pool = crossbeam_workstealing_pool::small_pool(4);
    let (command_tx, command_rx) = cb_unbounded();
    let dispatcher = Arc::new(WorkStealingPoolDispatcher { pool, command_tx });

    let mailbox_fn = Arc::new(cbe_unbounded_mailbox_fn());

    let dispatcher_thread_dispatcher = dispatcher.to_owned();
    let _ = thread::spawn(move || dispatcher_thread_dispatcher.start(command_rx));

    let my_actor = ActorContext::<Request>::new(
        || Box::new(MyActor {}),
        dispatcher.to_owned(),
        mailbox_fn.to_owned(),
    );

    c.bench_function(
        "send messages to other actors using Crossbeam and Executors with unbounded channels",
        |b| {
            b.iter(|| {
                let mut ask_receiver = my_actor.actor_ref.ask(
                    &|reply_to| Request {
                        reply_to: reply_to.to_owned(),
                    },
                    mailbox_fn.to_owned(),
                );
                let _ = ask_receiver
                    .receiver_impl
                    .as_any()
                    .downcast_ref::<CbReceiver<AnyMessage>>()
                    .unwrap()
                    .recv()
                    .unwrap()
                    .downcast_ref::<Reply>()
                    .unwrap();
            });
        },
    );
}

fn send_messages_to_other_actors_bounded_tk(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (command_tx, command_rx) = tk_channel(1);
    let dispatcher = Arc::new(TokioDispatcher { command_tx });
    let mailbox_fn = Arc::new(tk_mailbox_fn(1));

    let my_actor = ActorContext::<Request>::new(
        || Box::new(MyActor {}),
        dispatcher.to_owned(),
        mailbox_fn.to_owned(),
    );

    let dispatcher_task_dispatcher = dispatcher.to_owned();
    let _ = rt.spawn(async move { dispatcher_task_dispatcher.start(command_rx).await });

    c.bench_function(
        "send messages to other actors using Tokio with bounded channels",
        |b| {
            b.to_async(&rt).iter(|| async {
                let mut ask_receiver = my_actor.actor_ref.ask(
                    &|reply_to| Request {
                        reply_to: reply_to.to_owned(),
                    },
                    mailbox_fn.to_owned(),
                );
                let _ = ask_receiver
                    .receiver_impl
                    .as_any()
                    .downcast_mut::<TkReceiver<AnyMessage>>()
                    .unwrap()
                    .recv()
                    .await
                    .unwrap()
                    .downcast_ref::<Reply>()
                    .unwrap();
            });
        },
    );
}

fn send_messages_to_other_actors_unbounded_tk(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (command_tx, command_rx) = tk_unbounded_channel();
    let dispatcher = Arc::new(TokioUnboundedDispatcher { command_tx });
    let mailbox_fn = Arc::new(tk_unbounded_mailbox_fn());

    let my_actor = ActorContext::<Request>::new(
        || Box::new(MyActor {}),
        dispatcher.to_owned(),
        mailbox_fn.to_owned(),
    );

    let dispatcher_task_dispatcher = dispatcher.to_owned();
    let _ = rt.spawn(async move { dispatcher_task_dispatcher.start(command_rx).await });

    c.bench_function(
        "send messages to other actors using Tokio with unbounded channels",
        |b| {
            b.to_async(&rt).iter(|| async {
                let mut ask_receiver = my_actor.actor_ref.ask(
                    &|reply_to| Request {
                        reply_to: reply_to.to_owned(),
                    },
                    mailbox_fn.to_owned(),
                );
                let _ = ask_receiver
                    .receiver_impl
                    .as_any()
                    .downcast_mut::<TkUnboundedReceiver<AnyMessage>>()
                    .unwrap()
                    .recv()
                    .await
                    .unwrap()
                    .downcast_ref::<Reply>()
                    .unwrap();
            });
        },
    );
}

fn create_new_actors_unbounded_cbe(c: &mut Criterion) {
    let pool = crossbeam_workstealing_pool::small_pool(4);
    let (command_tx, command_rx) = cb_unbounded();
    let dispatcher = Arc::new(WorkStealingPoolDispatcher { pool, command_tx });

    let mailbox_fn = Arc::new(cbe_unbounded_mailbox_fn());

    let dispatcher_thread_dispatcher = dispatcher.to_owned();
    let _ = thread::spawn(move || dispatcher_thread_dispatcher.start(command_rx));

    c.bench_function(
        "create new actors using Crossbeam and Executors with unbounded channels",
        |b| {
            b.iter(|| {
                let _ = ActorContext::<Request>::new(
                    || Box::new(MyActor {}),
                    dispatcher.to_owned(),
                    mailbox_fn.to_owned(),
                );
            });
        },
    );
}

fn create_new_actors_unbounded_tk(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (command_tx, command_rx) = tk_unbounded_channel();
    let dispatcher = Arc::new(TokioUnboundedDispatcher { command_tx });
    let mailbox_fn = Arc::new(tk_unbounded_mailbox_fn());

    let dispatcher_task_dispatcher = dispatcher.to_owned();
    let _ = rt.spawn(async move { dispatcher_task_dispatcher.start(command_rx).await });

    c.bench_function(
        "create new actors using Tokio with unbounded channels",
        |b| {
            b.to_async(&rt).iter(|| async {
                let _ = ActorContext::<Request>::new(
                    || Box::new(MyActor {}),
                    dispatcher.to_owned(),
                    mailbox_fn.to_owned(),
                );
            });
        },
    );
}

criterion_group!(
    benches,
    send_messages_to_other_actors_bounded_cbe,
    send_messages_to_other_actors_unbounded_cbe,
    send_messages_to_other_actors_bounded_tk,
    send_messages_to_other_actors_unbounded_tk,
    create_new_actors_unbounded_cbe,
    create_new_actors_unbounded_tk,
);
criterion_main!(benches);

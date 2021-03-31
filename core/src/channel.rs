use alloc::boxed::Box;
use core::any::Any;
use core::fmt;

/// Provides an abstraction over any type of channel so that the core actor library
/// can function.
///
/// The primary declarations and their doc were copied from Crossbeam but appear
/// to apply across popular runtimes including Tokio and async-std.

/// The receiving side of a channel.
pub struct Receiver<T> {
    pub receiver_impl: Box<dyn ReceiverImpl<Item = T>>,
}

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Receiver { .. }")
    }
}

/// Required to be implemented by the provider of a channel
pub trait ReceiverImpl {
    type Item;
    /// Return self as an Any so that it can be downcast
    fn as_any(&self) -> &dyn Any;
}

/// An error returned from the [`recv`] method.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "receiving on an empty and disconnected channel".fmt(f)
    }
}

/// An error returned from the [`recv_timeout`] method.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum RecvTimeoutError {
    /// A message could not be received because the channel is empty and the operation timed out.
    ///
    /// If this is a zero-capacity channel, then the error indicates that there was no sender
    /// available to send a message and the operation timed out.
    Timeout,

    /// The message could not be received because the channel is empty and disconnected.
    Disconnected,
}

impl fmt::Display for RecvTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            RecvTimeoutError::Timeout => "timed out waiting on receive operation".fmt(f),
            RecvTimeoutError::Disconnected => "channel is empty and disconnected".fmt(f),
        }
    }
}

/// The sending side of a channel.
pub struct Sender<T> {
    pub sender_impl: Box<dyn SenderImpl<Item = T>>,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> Sender<T> {
    /// Attempts to send a message into the channel without blocking.
    ///
    /// This method will either send a message into the channel immediately or return an error if
    /// the channel is full or disconnected. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will send the message only if there
    /// happens to be a receive operation on the other side of the channel at the same time.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        self.sender_impl.try_send(msg)
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            sender_impl: self.sender_impl.clone(),
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Sender { .. }")
    }
}

/// Required to be implemented by the provider of a channel
pub trait SenderImpl {
    type Item;
    // Provide a cloning function
    fn clone(&self) -> Box<dyn SenderImpl<Item = Self::Item>>;
    // Provide a try_send function
    fn try_send(&self, msg: Self::Item) -> Result<(), TrySendError<Self::Item>>;
}

/// An error returned from the [`try_send`] method.
///
/// The error contains the message being sent so it can be recovered.
///
/// [`try_send`]: super::Sender::try_send
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T> {
    /// The message could not be sent because the channel is full.
    ///
    /// If this is a zero-capacity channel, then the error indicates that there was no receiver
    /// available to receive the message at the time.
    Full(T),

    /// The message could not be sent because the channel is disconnected.
    Disconnected(T),
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => "sending on a full channel".fmt(f),
            TrySendError::Disconnected(..) => "sending on a disconnected channel".fmt(f),
        }
    }
}

/// An error returned from the [`try_recv`] method.
///
/// [`try_recv`]: super::Receiver::try_recv
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError {
    /// A message could not be received because the channel is empty.
    ///
    /// If this is a zero-capacity channel, then the error indicates that there was no sender
    /// available to send a message at the time.
    Empty,

    /// The message could not be received because the channel is empty and disconnected.
    Disconnected,
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TryRecvError::Empty => "receiving on an empty channel".fmt(f),
            TryRecvError::Disconnected => "receiving on an empty and disconnected channel".fmt(f),
        }
    }
}

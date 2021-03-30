use alloc::boxed::Box;
use alloc::sync::Arc;
use core::any::Any;
use core::fmt;
use core::marker::PhantomData;

/// The receiving side of a channel.
pub struct Receiver<T> {
    pub phantom_marker: PhantomData<T>,
    pub underlying: Box<dyn Any>,
}

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

/// An error returned from the [`recv`] method.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
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
        todo!()
    }
}

pub trait SenderImpl<T> {
    fn clone(&self) -> Box<dyn SenderImpl<T>>;
    fn send(&self, msg: T) -> Result<(), SendError<T>>;
}

/// The sending side of a channel.
pub struct Sender<T> {
    pub phantom_marker: PhantomData<T>,
    pub sender_impl: Arc<dyn SenderImpl<T>>,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> Sender<T> {
    /// Blocks the current thread until a message is sent or the channel is disconnected.
    ///
    /// If the channel is full and not disconnected, this call will block until the send operation
    /// can proceed. If the channel becomes disconnected, this call will wake up and return an
    /// error. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        self.sender_impl.send(msg)
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            phantom_marker: PhantomData,
            sender_impl: self.sender_impl.clone(),
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// An error returned from the [`send`] method.
///
/// The message could not be sent because the channel is disconnected.
///
/// The error contains the message so it can be recovered.
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
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
        todo!()
    }
}

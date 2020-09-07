use futures::{
    io::{AsyncRead, AsyncWrite},
    task::noop_waker,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{Error, Write},
    pin::Pin,
    rc::Rc,
    result::Result,
    task::{Context, Poll},
};

pub fn uncompress_data_read(source: impl AsyncRead) -> impl AsyncRead {
    // ...
    source
    // ...
}

const BUF_SIZE: usize = 1024;

struct AsyncReadImpl {
    read_data: Rc<RefCell<VecDeque<u8>>>,
}

impl AsyncRead for AsyncReadImpl {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        mut buf: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        if self.read_data.borrow().is_empty() {
            Poll::Pending
        } else {
            let len = usize::min(self.read_data.borrow().len(), buf.len());
            for _ in 0..len {
                buf.write_all(&[self.read_data.borrow_mut().pop_front().unwrap()])
                    .unwrap();
            }
            Poll::Ready(Ok(len))
        }
    }
}

struct AsyncWriteImpl<W: AsyncWrite + Unpin> {
    target: RefCell<W>,
    read_data: Rc<RefCell<VecDeque<u8>>>,
    read_result: Pin<Box<dyn AsyncRead>>,
    backlog: Vec<u8>,
    pos: usize,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for AsyncWriteImpl<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        let mut this = Pin::into_inner(self);
        match Pin::new(&mut this).poll_flush(cx) {
            Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
            Poll::Pending => return Poll::Pending,
            _ => {}
        }
        for &i in buf {
            this.read_data.borrow_mut().push_back(i);
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if self.backlog.len() != self.pos {
            let poll =
                Pin::new(&mut *self.target.borrow_mut()).poll_write(cx, &self.backlog[self.pos..]);
            match poll {
                Poll::Ready(Ok(0)) => return Poll::Ready(Ok(())),
                Poll::Ready(Ok(n)) => self.pos += n,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            };
        }
        let mut data = [0; BUF_SIZE];
        match self
            .read_result
            .as_mut()
            .poll_read(&mut Context::from_waker(&noop_waker()), &mut data)
        {
            Poll::Pending => Poll::Ready(Ok(())),
            Poll::Ready(Ok(0)) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(n)) => {
                self.backlog = data[0..n].to_owned();
                self.pos = 0;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.poll_flush(cx)
    }
}

pub fn uncompress_data_write(target: impl AsyncWrite + Unpin) -> impl AsyncWrite {
    let read_data = Rc::new(RefCell::new(VecDeque::new()));
    AsyncWriteImpl {
        target: RefCell::new(target),
        read_data: read_data.clone(),
        read_result: Box::pin(uncompress_data_read(AsyncReadImpl { read_data })),
        backlog: vec![],
        pos: 0,
    }
}

#[test]
fn simple() {
    use futures::{executor::block_on, io::Cursor};

    let mut input = (0..=255)
        .cycle()
        .take(1024 * 1024 + 123)
        .collect::<Vec<_>>();
    let mut output = vec![];
    block_on(futures::io::copy(
        Cursor::new(&mut input),
        &mut uncompress_data_write(&mut output),
    ))
    .unwrap();
    assert_eq!(input, output);
}

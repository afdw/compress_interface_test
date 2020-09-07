use futures::{
    io::{AsyncRead, AsyncWrite, Cursor},
    task::noop_waker,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::Error,
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
    read_data: Rc<RefCell<VecDeque<Cursor<Vec<u8>>>>>,
}

impl AsyncRead for AsyncReadImpl {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        while !self.read_data.borrow().is_empty() {
            let mut read_data = self.read_data.borrow_mut();
            let data = read_data.front_mut().unwrap();
            let res = Pin::new(&mut *data).poll_read(cx, buf);
            if let Poll::Ready(Ok(0)) = res {
                read_data.pop_front();
            } else {
                return res;
            }
        }
        Poll::Pending
    }
}

struct AsyncWriteImpl<W: AsyncWrite + Unpin> {
    target: RefCell<W>,
    read_data: Rc<RefCell<VecDeque<Cursor<Vec<u8>>>>>,
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
        this.read_data
            .borrow_mut()
            .push_back(Cursor::new(buf.to_owned()));
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
            Poll::Ready(Ok(0)) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(n)) => {
                self.backlog = data[0..n].to_owned();
                self.pos = 0;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Ready(Ok(())),
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
    let mut input = (0..=255)
        .cycle()
        .take(1024 * 1024 + 123)
        .collect::<Vec<_>>();
    let mut output = vec![];
    futures::executor::block_on(futures::io::copy(
        Cursor::new(&mut input),
        &mut uncompress_data_write(&mut output),
    ))
    .unwrap();
    assert_eq!(input, output);
}

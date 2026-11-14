//! PostgreSQL wire input gate: validate each five-byte backend header before the driver
//! can reserve memory from an untrusted advertised frame length. No TLS/remote transport.
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub struct Wire {
    inner: Box<dyn Io>,
    header: [u8; 5],
    read: usize,
    sent: usize,
    body: usize,
    total: usize,
    pub limited: Arc<AtomicBool>,
}
impl Wire {
    pub async fn open(config: &super::Config, limited: Arc<AtomicBool>) -> Result<Self, super::Error> {
        let inner: Box<dyn Io> = if config.host.starts_with('/') {
            #[cfg(unix)]
            {
                Box::new(
                    tokio::net::UnixStream::connect(std::path::Path::new(&config.host).join(format!(".s.PGSQL.{}", config.port)))
                        .await
                        .map_err(super::Error::from)?,
                )
            }
            #[cfg(not(unix))]
            {
                return Err(super::Error::Input);
            }
        } else {
            let address = if config.host == "localhost" {
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            } else {
                config.host.parse::<std::net::IpAddr>().map_err(|_| super::Error::Input)?
            };
            if !address.is_loopback() {
                return Err(super::Error::Input);
            }
            Box::new(tokio::net::TcpStream::connect((address, config.port)).await.map_err(super::Error::from)?)
        };
        Ok(Self { inner, header: [0; 5], read: 0, sent: 0, body: 0, total: 0, limited })
    }
    fn limit(&self) -> io::Error {
        self.limited.store(true, Ordering::Relaxed);
        io::Error::new(io::ErrorKind::InvalidData, "PostgreSQL response exceeded the wire limit")
    }
}
impl AsyncRead for Wire {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if self.read == 5 && self.sent == 5 && self.body == 0 {
                self.read = 0;
                self.sent = 0;
            }
            while self.read < 5 {
                let at = self.read;
                let mut bytes = [0u8; 5];
                let mut buffer = ReadBuf::new(&mut bytes[..5 - at]);
                match Pin::new(&mut *self.inner).poll_read(cx, &mut buffer) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(())) => {}
                }
                let count = buffer.filled().len();
                if count == 0 {
                    return if at == 0 { Poll::Ready(Ok(())) } else { Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof))) };
                }
                self.header[at..at + count].copy_from_slice(buffer.filled());
                self.read += count;
                self.total += count;
                if self.total > 8 * 1024 * 1024 {
                    return Poll::Ready(Err(self.limit()));
                }
            }
            if self.sent == 0 {
                let length = u32::from_be_bytes([self.header[1], self.header[2], self.header[3], self.header[4]]) as usize;
                if !(4..=2 * 1024 * 1024).contains(&length) {
                    return Poll::Ready(Err(self.limit()));
                }
                self.body = length - 4;
            }
            if self.sent < 5 {
                let count = out.remaining().min(5 - self.sent);
                out.put_slice(&self.header[self.sent..self.sent + count]);
                self.sent += count;
                return Poll::Ready(Ok(()));
            }
            if self.body > 0 {
                if self.total >= 8 * 1024 * 1024 {
                    return Poll::Ready(Err(self.limit()));
                }
                let count = out.remaining().min(self.body).min(8192).min(8 * 1024 * 1024 - self.total);
                let mut bytes = [0u8; 8192];
                let mut buffer = ReadBuf::new(&mut bytes[..count]);
                match Pin::new(&mut *self.inner).poll_read(cx, &mut buffer) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(())) => {}
                }
                let count = buffer.filled().len();
                if count == 0 {
                    return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
                }
                self.total += count;
                self.body -= count;
                out.put_slice(buffer.filled());
                return Poll::Ready(Ok(()));
            }
        }
    }
}
impl AsyncWrite for Wire {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[test]
    fn postgres_wire_checks_frame_length_before_exposing_a_header_to_driver() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (writer, reader) = tokio::io::duplex(64);
            let limited = Arc::new(AtomicBool::new(false));
            let mut wire = Wire { inner: Box::new(reader), header: [0; 5], read: 0, sent: 0, body: 0, total: 0, limited: limited.clone() };
            let task = tokio::spawn(async move {
                let mut writer = writer;
                writer.write_all(&[b'D', 0x7f, 0xff, 0xff, 0xff]).await.unwrap();
            });
            let mut bytes = Vec::new();
            assert!(wire.read_to_end(&mut bytes).await.is_err());
            assert!(bytes.is_empty());
            assert!(limited.load(Ordering::Relaxed));
            task.await.unwrap();
            let (writer, reader) = tokio::io::duplex(64);
            let limited = Arc::new(AtomicBool::new(false));
            let mut wire = Wire { inner: Box::new(reader), header: [0; 5], read: 0, sent: 0, body: 0, total: 0, limited: limited.clone() };
            let task = tokio::spawn(async move {
                let mut writer = writer;
                writer.write_all(&[b'Z', 0, 0, 0, 5, b'I']).await.unwrap();
            });
            let mut bytes = Vec::new();
            wire.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, [b'Z', 0, 0, 0, 5, b'I']);
            assert!(!limited.load(Ordering::Relaxed));
            task.await.unwrap();
        });
    }
}

use flume::{Receiver, Sender, bounded};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct Pool {
    tx: Sender<tokio::net::TcpStream>,
    pub rx: Receiver<tokio::net::TcpStream>,
    req: String,
}

impl Pool {
    pub async fn new(host: String, path: String, max: usize) -> Self {
        let (tx, rx) = bounded(max);

        for _ in 0..max {
            let conn = tokio::net::TcpStream::connect(&host).await.unwrap();
            let _ = tx.send_async(conn).await.unwrap();
        }

        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n\r\n",
            path, host
        );

        Pool { tx, rx: rx, req }
    }

    #[inline(always)]
    pub async fn get(&self) -> Option<tokio::net::TcpStream> {
        if let Ok(conn) = self.rx.try_recv() {
            Some(conn)
        } else {
            None
        }
    }

    #[inline(always)]
    pub async fn put(&self, conn: tokio::net::TcpStream) {
        let _ = self.tx.send_async(conn).await.unwrap();
    }

    #[inline(always)]
    pub async fn send_get(&self) -> (bool, Option<tokio::net::TcpStream>) {
        if let Some(mut conn) = self.get().await {
            conn.write_all(self.req.as_bytes()).await.unwrap();

            let mut buffer = [0; 1024];
            if let Ok(n) = conn.read(&mut buffer).await {
                let mut headers = [httparse::EMPTY_HEADER; 32];
                let mut response = httparse::Response::new(&mut headers);

                let _ = response.parse(&buffer[..n]);

                if response.code == Some(200) {
                    return (true, Some(conn));
                }

                (false, Some(conn))
            } else {
                (false, Some(conn))
            }
        } else {
            (false, None)
        }
    }

    pub async fn tes(&self) -> u64 {
        let start = tokio::time::Instant::now();
        let (ok, conn_opt) = self.send_get().await;

        if ok && let Some(conn) = conn_opt {
            self.put(conn).await;
        }
        let time = start.elapsed().as_millis() as u64;
        time
    }
}

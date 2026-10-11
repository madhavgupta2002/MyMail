use mymail_server::db::Db;
use mymail_server::smtp;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

struct Client {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Client {
    async fn connect(addr: std::net::SocketAddr) -> Self {
        let (r, w) = TcpStream::connect(addr).await.unwrap().into_split();
        let mut c = Client {
            reader: BufReader::new(r),
            writer: w,
        };
        assert!(c.read_reply().await.starts_with("220"));
        c
    }

    /// Reads a (possibly multi-line) reply and returns the last line.
    async fn read_reply(&mut self) -> String {
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).await.unwrap();
            if line.len() < 4 || line.as_bytes()[3] != b'-' {
                return line.trim_end().to_string();
            }
        }
    }

    async fn cmd(&mut self, line: &str) -> String {
        self.writer
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .unwrap();
        self.read_reply().await
    }
}

async fn start() -> (Db, std::net::SocketAddr) {
    let db = Db::open_in_memory().unwrap();
    db.create_account("Me", "me@mymail.local").unwrap();
    db.create_account("Alice", "alice@mymail.local").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(smtp::serve(listener, db.clone()));
    (db, addr)
}

#[tokio::test]
async fn delivers_message_to_local_recipients() {
    let (db, addr) = start().await;
    let mut c = Client::connect(addr).await;

    assert!(c.cmd("EHLO test").await.starts_with("250 "));
    assert!(c.cmd("RCPT TO:<me@mymail.local>").await.starts_with("503"));
    assert!(
        c.cmd("MAIL FROM:<carol@example.com> SIZE=100")
            .await
            .starts_with("250")
    );
    assert!(c.cmd("RCPT TO:<me@mymail.local>").await.starts_with("250"));
    assert!(
        c.cmd("RCPT TO:<Alice@MyMail.local>")
            .await
            .starts_with("250")
    );
    assert!(
        c.cmd("RCPT TO:<nobody@mymail.local>")
            .await
            .starts_with("550")
    );
    assert!(c.cmd("DATA").await.starts_with("354"));
    let body = "From: Carol <carol@example.com>\r\n\
                To: me@mymail.local, alice@mymail.local\r\n\
                Subject: Via SMTP\r\n\
                \r\n\
                Hello over SMTP.\r\n\
                ..leading dot line\r\n\
                .";
    let reply = c.cmd(body).await;
    assert!(reply.starts_with("250"), "{reply}");
    assert!(c.cmd("NOOP").await.starts_with("250"));
    assert!(c.cmd("VRFY me").await.starts_with("502"));
    assert!(c.cmd("QUIT").await.starts_with("221"));

    let me = db
        .find_account_by_email("me@mymail.local")
        .unwrap()
        .unwrap();
    let inbox = db.list_messages(me.id, "inbox", None).unwrap();
    assert_eq!(inbox.len(), 1);
    let msg = db.get_message(inbox[0].id).unwrap();
    assert_eq!(msg.subject, "Via SMTP");
    assert_eq!(msg.from_name, "Carol");
    assert_eq!(msg.from_addr, "carol@example.com");
    assert_eq!(msg.to_addrs, "me@mymail.local, alice@mymail.local");
    assert!(msg.body.contains("Hello over SMTP."));
    assert!(
        msg.body.contains("\n.leading dot line"),
        "dot-unstuffed: {:?}",
        msg.body
    );
    assert!(!msg.is_read);

    let alice = db
        .find_account_by_email("alice@mymail.local")
        .unwrap()
        .unwrap();
    assert_eq!(db.list_messages(alice.id, "inbox", None).unwrap().len(), 1);
}

#[tokio::test]
async fn rejects_bad_sequences_and_oversize() {
    let (db, addr) = start().await;
    let mut c = Client::connect(addr).await;
    assert!(c.cmd("HELO x").await.starts_with("250"));
    assert!(c.cmd("DATA").await.starts_with("503"));
    assert!(c.cmd("MAIL FROM:bad").await.starts_with("501"));
    assert!(
        c.cmd("MAIL FROM:<a@b.c> SIZE=99999999999")
            .await
            .starts_with("552")
    );
    assert!(c.cmd("MAIL FROM:<a@b.c>").await.starts_with("250"));
    assert!(c.cmd("RCPT TO:<me@mymail.local>").await.starts_with("250"));
    assert!(c.cmd("RSET").await.starts_with("250"));
    assert!(
        c.cmd("DATA").await.starts_with("503"),
        "RSET clears recipients"
    );

    // A message larger than the limit is consumed and rejected.
    assert!(c.cmd("MAIL FROM:<a@b.c>").await.starts_with("250"));
    assert!(c.cmd("RCPT TO:<me@mymail.local>").await.starts_with("250"));
    assert!(c.cmd("DATA").await.starts_with("354"));
    let chunk = "x".repeat(999) + "\r\n";
    let big = chunk.repeat(smtp::MAX_MESSAGE_SIZE / chunk.len() + 10);
    c.writer.write_all(big.as_bytes()).await.unwrap();
    assert!(c.cmd(".").await.starts_with("552"));
    // Session still usable afterwards.
    assert!(c.cmd("NOOP").await.starts_with("250"));

    let me = db
        .find_account_by_email("me@mymail.local")
        .unwrap()
        .unwrap();
    assert!(db.list_messages(me.id, "inbox", None).unwrap().is_empty());
}

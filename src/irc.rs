use std::net::{TcpStream, SocketAddr};
use std::io::{self, Read, Write};
use std::time::Duration;
use std::ops::Range;
use std::fmt;


pub struct IrcClient {
    stream: TcpStream,
    data: Vec<u8>,
}

impl IrcClient {

    pub fn connect(addr: &SocketAddr) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(addr, Duration::from_secs(2))?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            data: Vec::new(),
        })
    }

    /// Receive raw data from the socket. To read the replies, 
    /// use [`read_reply`].
    pub fn recv(&mut self) -> io::Result<()> {

        let mut buf = [0; 64];

        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(size) => self.data.extend_from_slice(&buf[..size]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        
        Ok(())

    }

    /// Send a raw command using a format.
    pub fn send_fmt(&mut self, fmt: fmt::Arguments) -> io::Result<()> {
        self.stream.write_fmt(fmt)?;
        self.stream.write_all(b"\r\n")
    }

    pub fn send_auth(&mut self, user: &str, token: &str) -> io::Result<()> {
        self.send_fmt(format_args!("PASS oauth:{token}"))?;
        self.send_fmt(format_args!("NICK {user}"))
    }

    /// Read a single reply from the internal raw data, read 
    /// using [`recv`].
    pub fn decode_reply(&mut self) -> Option<IrcReply> {

        while let Some(cr_pos) = self.data.iter().position(|&b| b == b'\r') {
            
            let reply = IrcReply::from_str(std::str::from_utf8(&self.data[..cr_pos]).unwrap());

            let end_pos = match self.data[cr_pos + 1] {
                b'\n' => cr_pos + 2,
                _ => cr_pos + 1,
            };

            self.data.drain(..end_pos);

            if reply.is_some() {
                return reply;
            }

        }

        None

    }

}


pub struct IrcReply {
    pub raw: String,
    pub command: IrcReplyCommand,
    metadata_range: Range<usize>,
    sender_range: Range<usize>,
    target_range: Range<usize>,
    text_start: usize,
}

#[derive(Debug)]
pub enum IrcReplyCommand {
    Raw(String),
    Welcome,
    YourHost,
    Created,
    MyInfo,
    MotdStart,
    MotdText,
    MotdStop,
    PrivMsg,
    Ping,
    Join,
    Name,
    EndOfNames,
}

pub struct IrcSender<'a> {
    pub nickname: Option<&'a str>,
    pub user: Option<&'a str>,
    pub server: &'a str,
}

impl IrcReply {

    pub fn from_str<S: Into<String>>(line: S) -> Option<Self> {

        let mut reply = IrcReply {
            raw: line.into(),
            command: IrcReplyCommand::Welcome,
            metadata_range: 0..0,
            sender_range: 0..0,
            target_range: 0..0,
            text_start: 0,
        };

        let mut start = 0;
        let mut offset = 0;

        for (index, part) in reply.raw.splitn(5, ' ').enumerate() {

            let index = index - start;

            if index == 0 && part.starts_with(':') {
                reply.sender_range = offset + 1..(offset  + part.len());
                start += 1;
            } else if index == 0 && part.starts_with('@') {
                reply.metadata_range = offset + 1..(offset  + part.len());
                start += 1;
            } else if index == 0 {

                reply.command = match part {
                    "001" => IrcReplyCommand::Welcome,
                    "002" => IrcReplyCommand::YourHost,
                    "003" => IrcReplyCommand::Created,
                    "004" => IrcReplyCommand::MyInfo,
                    "375" => IrcReplyCommand::MotdStart,
                    "372" => IrcReplyCommand::MotdText,
                    "376" => IrcReplyCommand::MotdStop,
                    "353" => IrcReplyCommand::Name,
                    "366" => IrcReplyCommand::EndOfNames,
                    "PRIVMSG" => IrcReplyCommand::PrivMsg,
                    "PING" => IrcReplyCommand::Ping,
                    "JOIN" => IrcReplyCommand::Join,
                    _ => IrcReplyCommand::Raw(part.to_string()),
                };

            } else {

                match reply.command {
                    IrcReplyCommand::Welcome |
                    IrcReplyCommand::YourHost |
                    IrcReplyCommand::Created |
                    IrcReplyCommand::MotdStart |
                    IrcReplyCommand::MotdText |
                    IrcReplyCommand::MotdStop |
                    IrcReplyCommand::PrivMsg => {
                        if index == 1 {
                            reply.target_range = offset..(offset + part.len());
                        } else if index == 2 {
                            if part.starts_with(':') {
                                reply.text_start = offset + 1;
                            }
                            break;
                        }
                    }
                    IrcReplyCommand::Join => {
                        if index == 1 {
                            reply.target_range = offset..(offset + part.len());
                        }
                        break;
                    }
                    IrcReplyCommand::Ping => {
                        if index == 1 && part.starts_with(':') {
                            reply.text_start = offset + 1;
                        }
                        break;
                    }
                    _ => break,
                }

            }

            offset += part.len() + 1;

        }

        Some(reply)

    }

    pub fn metadata(&self) -> Option<&str> {
        if self.metadata_range.is_empty() {
            None
        } else {
            Some(&self.raw[self.metadata_range.clone()])
        }
    }

    pub fn sender(&self) -> Option<IrcSender> {

        if self.sender_range.is_empty() {
            return None;
        }

        let raw = &self.raw[self.sender_range.clone()];

        if let Some((raw, server)) = raw.split_once('@') {
            if let Some((nickname, user)) = raw.split_once('!') {
                Some(IrcSender { nickname: Some(nickname), user: Some(user), server })
            } else {
                Some(IrcSender { nickname: Some(raw), user: None, server })
            }
        } else {
            Some(IrcSender { nickname: None, user: None, server: raw })
        }

    }

    pub fn target(&self) -> Option<&str> {
        if self.target_range.is_empty() {
            None
        } else {
            Some(&self.raw[self.target_range.clone()])
        }
    }

    pub fn text(&self) -> Option<&str> {
        if self.text_start == 0 {
            None
        } else {
            Some(&self.raw[self.text_start..])
        }
    }

}

impl fmt::Debug for IrcReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("IrcReply");
        dbg.field("command", &self.command);
        if let Some(metadata) = self.metadata() {
            dbg.field("metadata", &metadata);
        }
        if let Some(sender) = self.sender() {
            dbg.field("sender", &sender);
        }
        if let Some(target) = self.target() {
            dbg.field("target", &target);
        }
        if let Some(text) = self.text() {
            dbg.field("text", &text);
        }
        dbg.finish()
    }
}

impl fmt::Debug for IrcSender<'_> {

    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("IrcSender");
        if let Some(nickname) = self.nickname {
            dbg.field("nickname", &nickname);
        }
        if let Some(user) = self.user {
            dbg.field("user", &user);
        }
        dbg.field("server", &self.server);
        dbg.finish()
    }

}
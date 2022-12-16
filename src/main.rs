use std::net::{SocketAddr, ToSocketAddrs};
use std::ops::{AddAssign, SubAssign};
use std::time::{Duration, Instant};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::fs::File;
use std::thread;
use std::env;
use std::fmt;

use chrono::Utc;

mod irc;
use irc::{IrcClient, IrcReplyCommand};

 
/// Duration of a single sample.
const SAMPLE_DURATION: Duration = Duration::from_millis(100);
/// Number of samples to keep for computing global averages.
const GLOBAL_SAMPLE_COUNT: usize = 100;
/// Full duration of the global sample.
const GLOBAL_SAMPLE_DURATION: Duration = Duration::from_millis(SAMPLE_DURATION.as_millis() as u64 * GLOBAL_SAMPLE_COUNT as u64);
/// Number of samples to keep for computing tpp averages,
/// used to choose which command to send.
const TPP_SAMPLE_COUNT: usize = 20;
/// Full duration of the TPP sample.
const TPP_SAMPLE_DURATION: Duration = Duration::from_millis(SAMPLE_DURATION.as_millis() as u64 * TPP_SAMPLE_COUNT as u64);

/// Interval in number of samples between each log of the
/// global sample.
const SAMPLE_LOG_INTERVAL: usize = 10;

/// The rate limit for sending messages (messages/s).
const MESSAGES_RATE_LIMIT: f32 = 20.0 / 30.0;


/// Internal function to print the interactive prompt.
fn print_prompt(fmt: fmt::Arguments, nl: bool) {
    print!("\r> {fmt}");
    if nl {
        println!();
    } else {
        std::io::stdout().flush().unwrap();
    }
}


fn main() {

    let addr_raw = env::var("TPP_ADDR").expect("missing TPP_ADDR variable");
    let token = env::var("TPP_TOKEN").expect("missing TPP_TOKEN variable");
    let user = env::var("TPP_USER").expect("missing TPP_USER variable");
    let channel = env::var("TPP_CHANNEL").expect("missing TPP_CHANNEL variable");
    let log_path_raw = env::var("TPP_LOG_PATH").expect("missing TPP_LOG_PATH variable");
    let bot = env::var("TPP_BOT").map(|s| s == "true").unwrap_or(false);

    let addr = addr_raw.to_socket_addrs().unwrap().next().unwrap();
    let log_path = log_path_raw.into();

    let config = Config {
        addr,
        user,
        token,
        channel,
        log_path,
        bot,
    };

    loop {
        if let Err(e) = run(&config) {
            print_prompt(format_args!("connection lost: {e:?}"), true);
        }
    }

}


fn run(config: &Config) -> io::Result<()> {

    print_prompt(format_args!("connect"), true);
    let mut irc = IrcClient::connect(&config.addr)?;

    print_prompt(format_args!("auth"), true);
    irc.send_auth(&config.user, &config.token)?;

    let mut log_file = File::options()
        .append(true)
        .create(true)
        .open(&config.log_path)?;

    // True when the server has sent a welcome command.
    let mut welcome = false;

    // Samples and time of the last slice.
    let mut samples = VecDeque::with_capacity(GLOBAL_SAMPLE_COUNT + 1);
    samples.push_back(Sample::default());
    // Start time of the active sample.
    let mut active_sample_time = Instant::now();

    // Used to average all samples.
    let mut global_sample = Sample::default();
    // Used to average all samples and choose most used TPP command.
    let mut tpp_sample = Sample::default();
    // Counter for the log samples.
    let mut log_interval = 0;

    // Last TPP command, used to switch between upper/lower 
    // case to avoid spam detection.
    let mut last_message = String::new();
    // Last send time.
    let mut next_message_time = Instant::now();
    // Number of messages sent since the beginning.
    let mut message_count = 0;

    loop {

        // In this section we check if the active sample needs to be flushed.
        // Using gt '>' because of the the last sample being the active one. 
        let samples_full = samples.len() > GLOBAL_SAMPLE_COUNT;
        let mut sample = samples.back_mut().unwrap();
        
        // If the active sample is long enough, flush it and count it in the
        // global sample.
        if active_sample_time.elapsed() > SAMPLE_DURATION {

            global_sample += sample;
            tpp_sample += sample;

            if samples_full {
                global_sample -= &samples.pop_front().unwrap();
            }

            // Using gt '>' because of the the last sample being the active one. 
            if samples.len() > TPP_SAMPLE_COUNT {
                tpp_sample -= samples.get(samples.len() - 1 - TPP_SAMPLE_COUNT).unwrap();
            }
            
            // Create a new active sample.
            samples.push_back(Sample::default());
            sample = samples.back_mut().unwrap();
            active_sample_time = Instant::now();

            // File logging.
            log_interval += 1;
            if log_interval >= SAMPLE_LOG_INTERVAL {

                let utc_time = Utc::now();
                log_interval = 0;

                if global_sample.tpp_command_count > 0 {
                    log_file.write_fmt(format_args!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", 
                        utc_time.timestamp(),
                        global_sample.message_count as f32 / GLOBAL_SAMPLE_DURATION.as_secs_f32(), 
                        global_sample.tpp_command_count as f32 / GLOBAL_SAMPLE_DURATION.as_secs_f32(),
                        global_sample.up as f32 / global_sample.tpp_command_count as f32,
                        global_sample.left as f32 / global_sample.tpp_command_count as f32,
                        global_sample.down as f32 / global_sample.tpp_command_count as f32,
                        global_sample.right as f32 / global_sample.tpp_command_count as f32,
                        global_sample.a as f32 / global_sample.tpp_command_count as f32,
                        global_sample.b as f32 / global_sample.tpp_command_count as f32,
                        global_sample.x as f32 / global_sample.tpp_command_count as f32,
                        global_sample.y as f32 / global_sample.tpp_command_count as f32,
                        global_sample.demo as f32 / global_sample.tpp_command_count as f32,
                        global_sample.anar as f32 / global_sample.tpp_command_count as f32,
                        global_sample.start as f32 / global_sample.tpp_command_count as f32,
                    )).unwrap();
                } else {
                    log_file.write_fmt(format_args!("{}\t{}\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n",  
                        utc_time.timestamp(),
                        global_sample.message_count as f32 / GLOBAL_SAMPLE_DURATION.as_secs_f32(), 
                    )).unwrap();
                }

                log_file.flush().unwrap();
                
            }
            
        }

        // In the following section, we take the most used command and send
        // it if enough time has passed.
        let tpp_command = tpp_sample.most_used();
                    
        // Compute the average number of command per second
        let tpp_command_sec = tpp_sample.tpp_command_count as f32
            / TPP_SAMPLE_DURATION.as_secs_f32();
        
        // Compute the ratio of commands/messages.
        let tpp_command_ratio = if tpp_sample.message_count == 0 { 0.0 } else {
            tpp_sample.tpp_command_count as f32 / tpp_sample.message_count as f32
        };

        // The real message interval is derived from the average interval.
        // We add 0.5s to the minimum interval as a margin of error.
        // If the minimum interval is not respected, the bot is ignored 
        // for 30 minutes by Twitch.
        let interval_secs = (8.0 - tpp_command_sec).max(1.0 / MESSAGES_RATE_LIMIT + 0.3);
        let interval = Duration::from_secs_f32(interval_secs);

        let remaining_time = if samples_full {
            if next_message_time >= Instant::now() {
                next_message_time - Instant::now()
            } else {
                Duration::from_secs(0)
            }
        } else {
            interval
        };

        let remaining_sec = remaining_time.as_secs_f32();
        print_prompt(format_args!("send {tpp_command:16} [in {remaining_sec:04.1}s, {tpp_command_sec:04.1} cmd/s, {tpp_command_ratio:.2} cmd/msg, {message_count:03} total]"), false);
        
        // Many condition are required to send a message, to avoid being caught as a bot.
        if config.bot && remaining_time.is_zero() && tpp_command_ratio >= 0.60 && tpp_command_sec >= 2.0 {

            println!();

            if last_message == tpp_command {
                last_message.make_ascii_uppercase();
            } else {
                last_message.clear();
                last_message.push_str(tpp_command);
            }
            
            irc.send_fmt(format_args!("PRIVMSG #{} :{last_message}", config.channel))?;
            message_count += 1;

            next_message_time = Instant::now() + interval;

        }

        // The following section receive replies and process them.
        irc.recv()?;
        while let Some(reply) = irc.decode_reply() {

            match reply.command {
                IrcReplyCommand::Welcome if !welcome => {
                    print_prompt(format_args!("join"), true);
                    irc.send_fmt(format_args!("JOIN #{}", config.channel))?;
                    welcome = true;
                }
                IrcReplyCommand::Ping => {
                    let text = reply.text().unwrap();
                    print_prompt(format_args!("pong '{text}'"), true);
                    irc.send_fmt(format_args!("PONG :{text}"))?;
                }
                IrcReplyCommand::PrivMsg if welcome => {

                    sample.message_count += 1;

                    let text = reply.text().unwrap();
                    let mut is_tpp_command = true;

                    if text.len() == 1 {
                        match text.chars().next().unwrap().to_ascii_lowercase() {
                            'u' | 'n' => sample.up += 1,
                            'l' | 'w' => sample.left += 1,
                            'd' | 's' => sample.down += 1,
                            'r' | 'e' => sample.right += 1,
                            'a' => sample.a += 1,
                            'b' => sample.b += 1,
                            'x' => sample.x += 1,
                            'y' => sample.y += 1,
                            _ => is_tpp_command = false,
                        }
                    } else {
                        match text {
                            "haut" | "HAUT" => sample.up += 1,
                            "gauche" | "GAUCHE" => sample.left += 1,
                            "bas" | "BAS" => sample.down += 1,
                            "droite" | "DROITE" => sample.right += 1,
                            "DÉMOCRATIE" | "DEMOCRATIE" |
                            "démocratie" | "democratie" => sample.demo += 1,
                            "ANARCHIE" | "anarchie" => sample.anar += 1,
                            "start" | "START" => sample.start += 1,
                            _ => is_tpp_command = false,
                        }
                    }

                    if is_tpp_command {
                        sample.tpp_command_count += 1;
                    }

                }
                _ => {
                    print_prompt(format_args!("received {:?}", reply), true);
                }
            }

        }

        thread::sleep(Duration::from_millis(10));

    }

}


#[derive(Debug)]
struct Config {
    addr: SocketAddr,
    user: String,
    token: String,
    channel: String,
    log_path: PathBuf,
    bot: bool,
}


#[derive(Debug, Default)]
struct Sample {
    message_count: u16,
    tpp_command_count: u16,
    up: u16,
    left: u16,
    down: u16,
    right: u16,
    a: u16,
    b: u16,
    x: u16,
    y: u16,
    demo: u16,
    anar: u16,
    start: u16,
}

impl Sample {

    fn most_used(&self) -> &str {

        let mut tpp_commands = [
            (self.up, "n"), 
            (self.left, "w"), 
            (self.down, "s"), 
            (self.right, "e"),
            (self.a, "a"),
            (self.b, "b"),
            (self.x, "x"),
            (self.y, "y"),
            (self.demo * 2, "democratie"),
            (self.anar / 4, "anarchie"),
            (self.start, "start"),
        ];

        tpp_commands.sort_by_key(|(n, _)| *n);
        tpp_commands[10].1
    
    }

}

impl<'a> AddAssign<&'a Self> for Sample {

    fn add_assign(&mut self, rhs: &'a Self) {
        self.message_count += rhs.message_count;
        self.tpp_command_count += rhs.tpp_command_count;
        self.up += rhs.up;
        self.left += rhs.left;
        self.down += rhs.down;
        self.right += rhs.right;
        self.a += rhs.a;
        self.b += rhs.b;
        self.x += rhs.x;
        self.y += rhs.y;
        self.demo += rhs.demo;
        self.anar += rhs.anar;
        self.start += rhs.start;
    }

}
impl<'a> SubAssign<&'a Self> for Sample {

    fn sub_assign(&mut self, rhs: &'a Self) {
        self.message_count -= rhs.message_count;
        self.tpp_command_count -= rhs.tpp_command_count;
        self.up -= rhs.up;
        self.left -= rhs.left;
        self.down -= rhs.down;
        self.right -= rhs.right;
        self.a -= rhs.a;
        self.b -= rhs.b;
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.demo -= rhs.demo;
        self.anar -= rhs.anar;
        self.start -= rhs.start;
    }

}

//! Linux app capture: one monitor record stream per playback stream of a shared
//! app, straight from PulseAudio / PipeWire (pipewire-pulse).
//!
//! A monitor stream (`direct_on_input`, what volume meters use) only listens: the
//! app keeps playing wherever it plays, at its own volume, and nothing is loaded
//! into or changed on the sound server. The streams belong to our connection, so
//! they are gone the moment sharing stops or OmniPanel exits, crash included.
//!
//! Apps open and close playback streams all the time (every track, every video),
//! so a subscription thread reports stream events and [`AppCapture::sync`] then
//! lines the record streams up with what is playing.

use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::executor::block_on;
use pulseaudio::protocol::{self, Command, Prop, SubscriptionEventFacility as Facility, SubscriptionMask};

use super::{FeedWriter, MAX_FEEDS, MixerPort, feed, same};
use crate::pulse::{CONNECT_TIMEOUT, CREATE_TIMEOUT, FRAGMENT_SECS, pulse_error, with_timeout};
use crate::{Result, VoiceError};

/// The sound server answers introspection at once; a stuck one must not hang the audio thread.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// PulseAudio and PipeWire can both do this.
pub(crate) fn unsupported() -> Option<&'static str> {
    None
}

pub(crate) struct AppCapture {
    /// First, so it is the first to go: deleting the record streams below must
    /// not come back as change events for a capture that is closing.
    _watcher: Watcher,
    /// Lists what is playing and which of our record streams the server still runs.
    server: Connection,
    /// Record streams by the sink input they monitor.
    streams: HashMap<u32, Stream>,
    /// Owns the record streams, and goes after them.
    client: Client,
    /// Marks our record streams on the server, see [`Stream::tag`].
    tag: String,
    rate: u32,
}

/// The connection the record streams run on, closed when dropped. Left to itself
/// the `pulseaudio` crate's client thread only ends once every handle is gone and
/// the server then sends something, which may never happen: shutting the socket
/// down ends it for sure, and with the connection whatever a late answer of the
/// server would still have started on it.
struct Client {
    client: pulseaudio::Client,
    _hangup: Hangup,
}

impl Client {
    fn connect() -> Result<Self> {
        let socket = server_socket()?;
        // Made first: a handshake that hangs is cut off too when this returns.
        let hangup = Hangup(socket.try_clone().map_err(io)?);
        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok());
        let connect = move || pulseaudio::Client::new_unix(c"OmniPanel", socket, cookie);
        let client = with_timeout(CONNECT_TIMEOUT, connect)?.map_err(|e| pulse_error(e, "server"))?;
        Ok(Self { client, _hangup: hangup })
    }
}

/// Shuts its connection down when dropped.
struct Hangup(UnixStream);

impl Drop for Hangup {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// A new connection to the sound server, before the handshake.
fn server_socket() -> Result<UnixStream> {
    let unavailable = || VoiceError::Backend("PulseAudio / PipeWire is not running".into());
    let path = pulseaudio::socket_path_from_env().ok_or_else(unavailable)?;
    UnixStream::connect(path).map_err(|_| unavailable())
}

impl AppCapture {
    /// `changed` runs on another thread whenever playback or record streams come,
    /// go or change; answer it with [`Self::sync`].
    pub fn open(mix_rate: u32, changed: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        static CAPTURES: AtomicU32 = AtomicU32::new(0);
        Ok(Self {
            _watcher: Watcher::spawn(changed)?,
            server: Connection::open()?,
            streams: HashMap::new(),
            client: Client::connect()?,
            tag: format!("{}.{}", std::process::id(), CAPTURES.fetch_add(1, Ordering::Relaxed)),
            rate: mix_rate,
        })
    }

    /// Record exactly the playback streams of `apps` that exist now; new feeds go to
    /// `port`. Returns the apps nothing is recorded of (not open, silent since they
    /// started, or their streams could not be recorded).
    ///
    /// Each stream is a request to the server: once `superseded` says the answer no
    /// longer matters (the list changed again, sharing is being stopped) no further
    /// one is made.
    pub fn sync(
        &mut self,
        apps: &[String],
        port: &mut MixerPort,
        superseded: &dyn Fn() -> bool,
    ) -> Result<Vec<String>> {
        let playing: Vec<Playback> = self.server.sink_inputs()?.iter().filter_map(Playback::of).collect();
        let wanted: HashSet<u32> = apps.iter().flat_map(|app| named(&playing, app)).map(|p| p.index).collect();
        // PulseAudio ends the monitor of a stream that moves to another sink (PipeWire
        // keeps it): what the server no longer runs is recorded again.
        let alive = self.server.record_streams(&self.tag)?;
        self.streams.retain(|index, stream| wanted.contains(index) && stream.alive(&alive, *index));
        let mut failed = None;
        for &index in &wanted {
            if self.streams.contains_key(&index) {
                continue;
            }
            if self.streams.len() == MAX_FEEDS {
                tracing::debug!("voice: sharing {MAX_FEEDS} streams already, ignoring more");
                break;
            }
            if superseded() {
                break;
            }
            let (writer, feed) = feed(self.rate, self.rate);
            // No answer at all ends the sync, and with it the capture (its owner lets
            // go of it, which hangs up): asking for the other streams as well would
            // wait as long for each, and a stop must not have to sit that out.
            match Stream::open(&self.client.client, index, &self.tag, self.rate, writer)? {
                // The mixer takes feeds every microphone callback; a full queue means it stalled.
                Ok(stream) if port.add(feed) => drop(self.streams.insert(index, stream)),
                Ok(_) => return Err(VoiceError::Backend("the microphone stream stalled".into())),
                // Streams end at any moment; the next event lines things up again.
                Err(VoiceError::DeviceNotFound(_)) => {}
                // One stream that cannot be recorded must not silence the others.
                Err(e) => failed = Some(e),
            }
        }
        port.drop_retired();
        let recorded = |app: &String| named(&playing, app).iter().any(|p| self.streams.contains_key(&p.index));
        let waiting = apps.iter().filter(|app| !recorded(app)).cloned().collect();
        match failed {
            Some(e) if self.streams.is_empty() => Err(e),
            Some(e) => {
                tracing::debug!("voice: could not record a stream of a shared app: {e}");
                Ok(waiting)
            }
            None => Ok(waiting),
        }
    }
}

/// One playback stream (sink input) and the app it belongs to.
#[derive(Debug, Clone, PartialEq)]
struct Playback {
    index: u32,
    /// The application's name as the Sound screen shows it, else its binary.
    name: String,
}

impl Playback {
    /// `None` for our own streams (the voice output must never feed itself) and
    /// for streams without an application.
    fn of(input: &protocol::SinkInputInfo) -> Option<Self> {
        let prop = |prop: Prop| text(input.props.get(prop)?);
        if prop(Prop::ApplicationProcessId).is_some_and(|pid| pid == std::process::id().to_string()) {
            return None;
        }
        let name = prop(Prop::ApplicationName).or_else(|| prop(Prop::ApplicationProcessBinary))?;
        Some(Self { index: input.index, name })
    }
}

/// A property value as text, if it has any.
fn text(value: &[u8]) -> Option<String> {
    let text = CStr::from_bytes_until_nul(value).ok()?.to_str().ok()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The streams of the app called `query`. Only [`Playback::name`] counts, never the
/// binary of an app that has a name: apps can share one (Electron), and a share
/// must stay with the app it names.
fn named<'a>(playing: &'a [Playback], query: &str) -> Vec<&'a Playback> {
    playing.iter().filter(|p| same(query, &p.name)).collect()
}

/// Stream property holding `<capture tag>:<sink input>`, so a capture finds its
/// own record streams in the server's list.
const TAG: &CStr = c"omnipanel.share";
/// A stream this young may not be in the server's list yet.
const SETTLE: Duration = Duration::from_secs(1);

/// A running monitor stream; dropping it stops the recording and ends its feed.
struct Stream {
    stream: pulseaudio::RecordStream,
    /// The feed's writer, shared with the record callback. The client only lets go
    /// of a callback when the server acknowledges the delete, which it refuses for
    /// a stream it already ended itself (the app closed it): taking the writer
    /// away here ends the feed either way.
    writer: Arc<Mutex<Option<FeedWriter>>>,
    opened: Instant,
}

impl Stream {
    /// Record sink input `index` as mono f32 at `rate` (the server downmixes and
    /// resamples), marked with the capture's `tag`. The outer error: the server
    /// did not answer. The inner one: it refused this stream.
    fn open(client: &pulseaudio::Client, index: u32, tag: &str, rate: u32, writer: FeedWriter) -> Result<Result<Self>> {
        let fragment_frames = ((rate as f32 * FRAGMENT_SECS) as u32).max(1);
        let mut props = protocol::Props::new();
        props.set(Prop::MediaName, c"Shared app sound");
        props.set_bytes(TAG, format!("{tag}:{index}\0"));
        let params = protocol::RecordStreamParams {
            sample_spec: protocol::SampleSpec {
                format: protocol::SampleFormat::Float32Le,
                channels: 1,
                sample_rate: rate,
            },
            channel_map: protocol::ChannelMap::mono(),
            direct_on_input_index: Some(index),
            buffer_attr: protocol::stream::BufferAttr { fragment_size: fragment_frames * 4, ..Default::default() },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            props,
            ..Default::default()
        };
        let writer = Arc::new(Mutex::new(Some(writer)));
        let feed = writer.clone();
        let callback = move |bytes: &[u8]| {
            // Never waits: the only other one to take the lock is `Drop`, once.
            if let Ok(mut feed) = feed.try_lock()
                && let Some(writer) = feed.as_mut()
            {
                let samples = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b));
                writer.write(bytes.len() / 4, samples);
            }
        };
        let creator = client.clone();
        let created = with_timeout(CREATE_TIMEOUT, move || block_on(creator.create_record_stream(params, callback)))?;
        Ok(created
            .map(|stream| Self { stream, writer, opened: Instant::now() })
            .map_err(|e| pulse_error(e, "app stream")))
    }

    /// Whether the server still runs this stream for sink input `index`
    /// (`alive` = the sink inputs it lists record streams of ours for).
    fn alive(&self, alive: &HashSet<u32>, index: u32) -> bool {
        alive.contains(&index) || self.opened.elapsed() < SETTLE
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // As for the microphone capture: queue the delete, never wait on the server.
        let _ = self.stream.clone().delete().now_or_never();
        self.writer.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

/// A blocking connection for what the `pulseaudio` client does not offer:
/// listing sink inputs and subscribing to events.
struct Connection {
    socket: BufReader<UnixStream>,
    version: u16,
    seq: u32,
}

impl Connection {
    fn open() -> Result<Self> {
        let socket = server_socket()?;
        socket.set_read_timeout(Some(REPLY_TIMEOUT)).map_err(io)?;
        socket.set_write_timeout(Some(REPLY_TIMEOUT)).map_err(io)?;
        let mut connection = Self { socket: BufReader::new(socket), version: protocol::MAX_VERSION, seq: 0 };

        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let auth =
            protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        let reply: protocol::AuthReply = connection.request(Command::Auth(auth))?;
        connection.version = reply.version.min(protocol::MAX_VERSION);
        let mut props = protocol::Props::new();
        props.set(Prop::ApplicationName, c"OmniPanel");
        let _: protocol::SetClientNameReply = connection.request(Command::SetClientName(props))?;
        Ok(connection)
    }

    fn send(&mut self, command: &Command) -> Result<()> {
        self.seq += 1;
        protocol::write_command_message(self.socket.get_mut(), self.seq, command, self.version).map_err(protocol_error)
    }

    fn request<R: protocol::CommandReply>(&mut self, command: Command) -> Result<R> {
        self.send(&command)?;
        let (seq, reply) = protocol::read_reply_message(&mut self.socket, self.version).map_err(protocol_error)?;
        if seq != self.seq {
            return Err(VoiceError::Backend("PulseAudio: reply out of order".into()));
        }
        Ok(reply)
    }

    fn sink_inputs(&mut self) -> Result<Vec<protocol::SinkInputInfo>> {
        self.request(Command::GetSinkInputInfoList)
    }

    /// Sink inputs the server runs a record stream of the capture `tag` for.
    fn record_streams(&mut self, tag: &str) -> Result<HashSet<u32>> {
        let outputs: Vec<protocol::SourceOutputInfo> = self.request(Command::GetSourceOutputInfoList)?;
        Ok(outputs.iter().filter_map(|output| recorded(&output.props, tag)).collect())
    }
}

/// The sink input a record stream with these properties monitors for the capture `tag`.
fn recorded(props: &protocol::Props, tag: &str) -> Option<u32> {
    text(props.get_bytes(TAG)?)?.strip_prefix(tag)?.strip_prefix(':')?.parse().ok()
}

/// The subscription thread: tells the owner about every playback or record stream
/// event, and once more when the server ends the connection.
struct Watcher {
    socket: UnixStream,
    thread: Option<JoinHandle<()>>,
    /// Set by `Drop`: the connection ending is then no news for the owner (who
    /// would answer by opening the capture again, at once).
    closing: Arc<AtomicBool>,
}

impl Watcher {
    fn spawn(changed: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let mut connection = Connection::open()?;
        connection.send(&Command::Subscribe(SubscriptionMask::SINK_INPUT | SubscriptionMask::SOURCE_OUTPUT))?;
        protocol::read_ack_message(&mut connection.socket).map_err(protocol_error)?;
        let socket = connection.socket.get_ref().try_clone().map_err(io)?;
        socket.set_read_timeout(None).map_err(io)?;
        let closing = Arc::new(AtomicBool::new(false));
        let closed = closing.clone();
        let thread = std::thread::Builder::new()
            .name("omni-voice-share".into())
            .spawn(move || {
                // Only events arrive on this connection from here on.
                while let Ok((_, command)) = protocol::read_command_message(&mut connection.socket, connection.version)
                {
                    if let Command::SubscribeEvent(event) = command
                        && matches!(event.event_facility, Facility::SinkInput | Facility::SourceOutput)
                    {
                        changed();
                    }
                }
                if !closed.load(Ordering::Acquire) {
                    changed();
                }
            })
            .map_err(io)?;
        Ok(Self { socket, thread: Some(thread), closing })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Ends the blocking read, and with it the thread.
        self.closing.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn io(err: std::io::Error) -> VoiceError {
    VoiceError::Backend(format!("PulseAudio: {err}"))
}

fn protocol_error(err: protocol::ProtocolError) -> VoiceError {
    match err {
        protocol::ProtocolError::ServerError(protocol::PulseError::NoEntity) => {
            VoiceError::DeviceNotFound("app stream".into())
        }
        other => VoiceError::Backend(format!("PulseAudio: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(index: u32, name: &str) -> Playback {
        Playback { index, name: name.into() }
    }

    #[test]
    fn a_share_stays_with_the_app_it_names() {
        // Firefox plays two streams; "Electron" is an app here, and also the binary of two others.
        let playing = [
            playback(60, "Slack"),
            playback(61, "Discord"),
            playback(62, "Electron"),
            playback(63, "Firefox"),
            playback(64, "Firefox"),
        ];
        let streams =
            |playing: &[Playback], query: &str| named(playing, query).iter().map(|p| p.index).collect::<Vec<_>>();
        assert_eq!(streams(&playing, "discord"), [61]);
        assert_eq!(streams(&playing, "Firefox "), [63, 64]);
        assert_eq!(streams(&playing, "Electron"), [62]);
        assert!(streams(&playing, "fire").is_empty() && streams(&playing, "").is_empty());
        // Once the app called Electron is gone nothing is taken in its place: it is waited for.
        assert!(streams(&playing[..2], "Electron").is_empty());
    }

    #[test]
    fn our_own_streams_are_never_captured() {
        let input = |pid: u32| {
            let mut props = protocol::Props::new();
            props.set(Prop::ApplicationName, c"Player");
            props.set(Prop::ApplicationProcessBinary, c"player");
            props.set_bytes(Prop::ApplicationProcessId.to_c_str(), format!("{pid}\0"));
            protocol::SinkInputInfo { index: 3, props, ..Default::default() }
        };
        assert_eq!(Playback::of(&input(1)), Some(playback(3, "Player")));
        assert_eq!(Playback::of(&input(std::process::id())), None);
    }

    #[test]
    fn a_capture_only_recognizes_its_own_record_streams() {
        let tagged = |value: &str| {
            let mut props = protocol::Props::new();
            props.set_bytes(TAG, format!("{value}\0"));
            props
        };
        assert_eq!(recorded(&tagged("41.0:87"), "41.0"), Some(87));
        // Another capture (an earlier one of ours, another OmniPanel) or no tag at all.
        assert_eq!(recorded(&tagged("41.1:87"), "41.0"), None);
        assert_eq!(recorded(&tagged("41.07:87"), "41.0"), None);
        assert_eq!(recorded(&protocol::Props::new(), "41.0"), None);
    }
}

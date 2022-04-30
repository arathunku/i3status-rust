//! The collection of blocks

pub mod prelude;

use crate::BoxedFuture;
use futures::future::FutureExt;
use serde::de::{self, Deserializer};
use serde::Deserialize;
use smallvec::SmallVec;
use smartstring::alias::String;
use tokio::sync::mpsc;
use toml::value::Table;

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use crate::click::{ClickHandler, MouseButton};
use crate::config::SharedConfig;
use crate::errors::*;
use crate::formatting::{value::Value, Format};
use crate::protocol::i3bar_event::I3BarEvent;
use crate::widget::State;
use crate::{Request, RequestCmd};

macro_rules! define_blocks {
    {
        $( $(#[cfg($attr: meta)])? $block: ident $(,)? )*
    } => {
        $(
            $(#[cfg($attr)])?
            pub mod $block;
        )*

        #[derive(Debug, Clone, Copy)]
        pub enum BlockType {
            $(
                $(#[cfg($attr)])?
                #[allow(non_camel_case_types)]
                $block,
            )*
        }

        impl BlockType {
            pub fn run(self, config: toml::Value, api: CommonApi) -> BlockFuture {
                let id = api.id;
                match self {
                    $(
                        $(#[cfg($attr)])?
                        Self::$block => {
                            $block::run(config, api).map(move |e| e.in_block(self, id)).boxed_local()
                        }
                    )*
                }
            }
        }

        impl<'de> Deserialize<'de> for BlockType {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct Visitor;

                impl<'de> de::Visitor<'de> for Visitor {
                    type Value = BlockType;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                        formatter.write_str("a block name")
                    }

                    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match v {
                            $(
                            $(#[cfg($attr)])?
                            stringify!($block) => Ok(BlockType::$block),
                            $(
                            #[cfg(not($attr))]
                            stringify!($block) => Err(E::custom(format!("Block '{}' has to be enabled at the compile time", stringify!($block)))),
                            )?
                            )*
                            unknown => Err(E::custom(format!("Unknown block '{unknown}'")))
                        }
                    }
                }

                deserializer.deserialize_str(Visitor)
            }
        }

    };
}

define_blocks!(
    apt,
    backlight,
    battery,
    bluetooth,
    cpu,
    custom,
    custom_dbus,
    disk_space,
    dnf,
    docker,
    external_ip,
    focused_window,
    github,
    hueshift,
    kdeconnect,
    load,
    #[cfg(feature = "maildir")]
    maildir,
    menu,
    memory,
    music,
    net,
    // networkmanager,
    notify,
    #[cfg(feature = "notmuch")]
    notmuch,
    nvidia_gpu,
    pacman,
    pomodoro,
    rofication,
    sound,
    speedtest,
    keyboard_layout,
    taskwarrior,
    temperature,
    time,
    toggle,
    uptime,
    watson,
    weather,
    xrandr,
);

pub type BlockFuture = BoxedFuture<Result<()>>;

#[derive(Debug, Clone, Copy)]
pub enum BlockEvent {
    Click(I3BarEvent),
    UpdateRequest,
}

pub struct CommonApi {
    pub id: usize,
    pub shared_config: SharedConfig,
    pub event_receiver: mpsc::Receiver<BlockEvent>,

    pub request_sender: mpsc::Sender<Request>,
    pub cmd_buf: SmallVec<[RequestCmd; 4]>,

    pub error_interval: Duration,
    pub error_format: Option<String>,
}

impl CommonApi {
    pub fn hide(&mut self) {
        self.cmd_buf.push(RequestCmd::Hide);
    }

    pub fn show(&mut self) {
        self.cmd_buf.push(RequestCmd::Show);
    }

    /// Receive the next event, such as click notification or update request.
    ///
    /// This method should be called regularly to avoid sender blocking. Currently, the runtime is
    /// single threaded, so full channel buffer will cause a deadlock. If receiving events is
    /// impossible / meaningless, call `event_receiver.close()`.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe.
    ///
    /// # Panics
    ///
    /// Panics if event sender is closed
    ///
    /// # Examples
    ///
    /// ```
    /// tokio::select! {
    ///     _ = timer.tick() => (),
    ///     UpdateRequest = api.event() => (),
    /// }
    /// ```
    pub async fn event(&mut self) -> BlockEvent {
        match self.event_receiver.recv().await {
            Some(event) => event,
            None => panic!("events stream ended"),
        }
    }

    pub fn set_icon(&mut self, icon: &str) -> Result<()> {
        let icon = if icon.is_empty() {
            String::new()
        } else {
            self.get_icon(icon)?
        };
        self.cmd_buf.push(RequestCmd::SetIcon(icon));
        Ok(())
    }

    pub fn set_icon_raw(&mut self, icon: String) {
        self.cmd_buf.push(RequestCmd::SetIcon(icon));
    }

    pub fn set_state(&mut self, state: State) {
        self.cmd_buf.push(RequestCmd::SetState(state));
    }

    pub fn set_text(&mut self, text: String) {
        self.cmd_buf.push(RequestCmd::SetText(text));
    }

    pub fn set_texts(&mut self, full: String, short: String) {
        self.cmd_buf.push(RequestCmd::SetTexts(full, short));
    }

    pub fn set_values(&mut self, values: HashMap<String, Value>) {
        self.cmd_buf.push(RequestCmd::SetValues(values));
    }

    pub fn set_format(&mut self, format: Format) {
        self.cmd_buf.push(RequestCmd::SetFormat(format));
    }

    pub fn set_full_screen(&mut self, value: bool) {
        self.cmd_buf.push(RequestCmd::SetFullScreen(value));
    }

    pub fn preserve(&mut self) {
        self.cmd_buf.push(RequestCmd::Preserve);
    }

    pub fn restore(&mut self) {
        self.cmd_buf.push(RequestCmd::Restore);
    }

    pub async fn get_dbus_connection(&mut self) -> Result<zbus::Connection> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.cmd_buf.push(RequestCmd::GetDbusConnection(sender));
        self.flush().await?;
        receiver.await.ok().error("Failed to get dbus connection")?
    }

    pub async fn get_system_dbus_connection(&mut self) -> Result<zbus::Connection> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.cmd_buf
            .push(RequestCmd::GetSystemDbusConnection(sender));
        self.flush().await?;
        receiver.await.ok().error("Failed to get dbus connection")?
    }

    pub async fn flush(&mut self) -> Result<()> {
        let cmds = std::mem::replace(&mut self.cmd_buf, SmallVec::new());
        self.request_sender
            .send(Request {
                block_id: self.id,
                cmds,
            })
            .await
            .error("Failed to send Request")?;
        Ok(())
    }

    pub fn get_icon(&self, icon: &str) -> Result<String> {
        self.shared_config.get_icon(icon)
    }

    pub async fn recoverable<Fn, Fut, T, E, Msg>(&mut self, mut f: Fn, msg: Msg) -> Result<T>
    where
        Fn: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: StdError,
        Msg: Clone + Into<String>,
    {
        let mut focused = false;
        let mut been_err = false;
        loop {
            match f().await {
                Ok(res) => {
                    if been_err {
                        // TODO restore hidden
                        // TODO restore the full screen properly
                        self.set_full_screen(false);
                        self.restore();
                    }
                    return Ok(res);
                }
                Err(err) => {
                    if !been_err {
                        self.preserve();
                        been_err = true;
                    }
                    let retry_at = tokio::time::Instant::now() + self.error_interval;

                    self.show();
                    self.set_state(State::Critical);

                    // TODO: do not toggle fullscreen if the block was already fullscreen before
                    // the error
                    loop {
                        if focused {
                            self.set_text(err.to_string().into());
                            self.set_full_screen(true);
                        } else {
                            self.set_text(
                                self.error_format
                                    .clone()
                                    .unwrap_or_else(|| msg.clone().into()),
                            );
                            self.set_full_screen(false);
                        }
                        self.flush().await?;

                        tokio::select! {
                            _ = tokio::time::sleep_until(retry_at) => break,
                            BlockEvent::Click(click) = self.event() => {
                                if click.button == MouseButton::Left {
                                    focused = !focused;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct CommonConfig {
    pub block: BlockType,

    #[serde(default)]
    pub click: ClickHandler,
    #[serde(default)]
    pub signal: Option<i32>,
    #[serde(default)]
    pub icons_format: Option<String>,
    #[serde(default)]
    pub theme_overrides: Option<HashMap<String, String>>,

    #[serde(default = "CommonConfig::default_error_interval")]
    pub error_interval: u64,
    #[serde(default)]
    pub error_format: Option<String>,
}

impl CommonConfig {
    fn default_error_interval() -> u64 {
        5
    }

    pub fn new(from: &mut toml::Value) -> Result<Self> {
        const FIELDS: &[&str] = &[
            "block",
            "click",
            "signal",
            "theme_overrides",
            "icons_format",
            "error_interval",
            "error_format",
        ];
        let mut common_table = Table::new();
        if let Some(table) = from.as_table_mut() {
            for &field in FIELDS {
                if let Some(it) = table.remove(field) {
                    common_table.insert(field.to_string(), it);
                }
            }
        }
        let common_value: toml::Value = common_table.into();
        CommonConfig::deserialize(common_value).config_error()
    }
}

//! A [pomodoro timer](https://en.wikipedia.org/wiki/Pomodoro_Technique)
//!
//! # Technique
//!
//! There are six steps in the original technique:
//! 1) Decide on the task to be done.
//! 2) Set the pomodoro timer (traditionally to 25 minutes).
//! 3) Work on the task.
//! 4) End work when the timer rings and put a checkmark on a piece of paper.
//! 5) If you have fewer than four checkmarks, take a short break (3–5 minutes) and then return to step 2.
//! 6) After four pomodoros, take a longer break (15–30 minutes), reset your checkmark count to zero, then go to step 1.
//!
//!
//! # Configuration
//!
//! Key | Values | Default
//! ----|--------|--------
//! `format` | A string to customise the output of this block. | <code>" $icon{ $message&vert;} "</code>
//! `message` | Message when timer expires | `"Pomodoro over! Take a break!"`
//! `break_message` | Message when break is over | `"Break over! Time to work!"`
//! `notify_cmd` | A shell command to run as a notifier. `{msg}` will be substituted with either `message` or `break_message`. | `None`
//! `blocking_cmd` | Is `notify_cmd` blocking? If it is, then pomodoro block will wait until the command finishes before proceeding. Otherwise, you will have to click on the block in order to proceed. | `false`
//!
//! Placeholder | Value                               | Type
//! ------------|-------------------------------------|------
//! `icon`      | A static icon                       | Icon
//! `message`   | Current message                     | Text
//!
//! # Example
//!
//! Use `swaynag` as a notifier:
//!
//! ```toml
//! [[block]]
//! block = "pomodoro"
//! notify_cmd = "swaynag -m '{msg}'"
//! blocking_cmd = true
//! ```
//!
//! Use `notify-send` as a notifier:
//!
//! ```toml
//! [[block]]
//! block = "pomodoro"
//! notify_cmd = "notify-send '{msg}'"
//! blocking_cmd = false
//! ```
//!
//! # Icons Used
//! - `pomodoro`
//! - `pomodoro_started`
//! - `pomodoro_stopped`
//! - `pomodoro_paused`
//! - `pomodoro_break`
//!
//! # TODO
//! - Use different icons.
//! - Use format strings.

use super::prelude::*;
use crate::subprocess::{spawn_shell, spawn_shell_sync};
use std::time::Instant;

#[derive(Deserialize, Debug, SmartDefault)]
#[serde(deny_unknown_fields, default)]
struct PomodoroConfig {
    format: FormatConfig,
    #[default("Pomodoro over! Take a break!".into())]
    message: String,
    #[default("Break over! Time to work!".into())]
    break_message: String,
    notify_cmd: Option<String>,
    blocking_cmd: bool,
}

struct Block {
    widget: Widget,
    api: CommonApi,
    block_config: PomodoroConfig,
}

impl Block {
    async fn set_text(&mut self, text: String) -> Result<()> {
        let mut values = map!(
            "icon" => Value::icon(self.api.get_icon("pomodoro")?),
        );
        if !text.is_empty() {
            values.insert("message".into(), Value::text(text));
        }
        self.widget.set_values(values);
        self.api.set_widget(&self.widget).await
    }

    async fn wait_for_click(&mut self, button: MouseButton) {
        loop {
            if let Click(click) = self.api.event().await {
                if click.button == button {
                    break;
                }
            }
        }
    }

    async fn read_params(&mut self) -> Result<(Duration, Duration, u64)> {
        let task_len = self.read_u64(25, "Task length:").await?;
        let break_len = self.read_u64(5, "Break length:").await?;
        let pomodoros = self.read_u64(4, "Pomodoros:").await?;
        Ok((
            Duration::from_secs(task_len * 60),
            Duration::from_secs(break_len * 60),
            pomodoros,
        ))
    }

    async fn read_u64(&mut self, mut number: u64, msg: &str) -> Result<u64> {
        loop {
            self.set_text(format!("{msg} {number}")).await?;
            if let Click(click) = self.api.event().await {
                match click.button {
                    MouseButton::Left => break,
                    MouseButton::WheelUp => number += 1,
                    MouseButton::WheelDown => number = number.saturating_sub(1),
                    _ => (),
                }
            }
        }
        Ok(number)
    }

    async fn run_pomodoro(
        &mut self,
        task_len: Duration,
        break_len: Duration,
        pomodoros: u64,
    ) -> Result<()> {
        for pomodoro in 0..pomodoros {
            // Task timer
            self.widget.state = State::Idle;
            let timer = Instant::now();
            loop {
                let elapsed = timer.elapsed();
                if elapsed >= task_len {
                    break;
                }
                let left = task_len - elapsed;
                let text = if pomodoro == 0 {
                    format!("{} min", (left.as_secs() + 59) / 60,)
                } else {
                    format!(
                        "{} {} min",
                        "|".repeat(pomodoro as usize),
                        (left.as_secs() + 59) / 60,
                    )
                };
                self.set_text(text).await?;
                select! {
                    _ = sleep(Duration::from_secs(10)) => (),
                    Click(click) = self.api.event() => {
                        if click.button == MouseButton::Middle {
                            return Ok(());
                        }
                    }
                }
            }

            // Show break message
            self.widget.state = State::Good;
            self.set_text(self.block_config.message.clone()).await?;
            if let Some(cmd) = &self.block_config.notify_cmd {
                let cmd = cmd.replace("{msg}", &self.block_config.message);
                if self.block_config.blocking_cmd {
                    spawn_shell_sync(&cmd)
                        .await
                        .error("failed to run notify_cmd")?;
                } else {
                    spawn_shell(&cmd).error("failed to run notify_cmd")?;
                    self.wait_for_click(MouseButton::Left).await;
                }
            } else {
                self.wait_for_click(MouseButton::Left).await;
            }

            // No break after the last pomodoro
            if pomodoro == pomodoros - 1 {
                break;
            }

            // Break timer
            let timer = Instant::now();
            loop {
                let elapsed = timer.elapsed();
                if elapsed >= break_len {
                    break;
                }
                let left = break_len - elapsed;
                self.set_text(format!("Break: {} min", (left.as_secs() + 59) / 60,))
                    .await?;
                select! {
                    _ = sleep(Duration::from_secs(10)) => (),
                    Click(click) = self.api.event() => {
                        if click.button == MouseButton::Middle {
                            return Ok(());
                        }
                    }
                }
            }

            // Show task message
            self.widget.state = State::Good;
            self.set_text(self.block_config.break_message.clone())
                .await?;
            if let Some(cmd) = &self.block_config.notify_cmd {
                let cmd = cmd.replace("{msg}", &self.block_config.break_message);
                if self.block_config.blocking_cmd {
                    spawn_shell_sync(&cmd)
                        .await
                        .error("failed to run notify_cmd")?;
                } else {
                    spawn_shell(&cmd).error("failed to run notify_cmd")?;
                    self.wait_for_click(MouseButton::Left).await;
                }
            } else {
                self.wait_for_click(MouseButton::Left).await;
            }
        }

        Ok(())
    }
}

pub async fn run(block_config: toml::Value, api: CommonApi) -> Result<()> {
    let block_config = PomodoroConfig::deserialize(block_config).config_error()?;
    let format = FormatConfig::default().with_default(" $icon{ $message|} ")?;
    let widget = api.new_widget().with_format(format);

    let mut block = Block {
        widget,
        api,
        block_config,
    };

    loop {
        // Send collaped block
        block.widget.state = State::Idle;
        block.set_text(String::new()).await?;

        // Wait for left click
        block.wait_for_click(MouseButton::Left).await;

        // Read params
        let (task_len, break_len, pomodoros) = block.read_params().await?;

        // Run!
        block.run_pomodoro(task_len, break_len, pomodoros).await?;
    }
}

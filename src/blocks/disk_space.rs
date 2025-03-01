//! Disk usage statistics
//!
//! # Configuration
//!
//! Key | Values | Default
//! ----|--------|--------
//! `path` | Path to collect information from. Supports path expansions e.g. `~`. | `"/"`
//! `interval` | Update time in seconds | `20`
//! `format` | A string to customise the output of this block. See below for available placeholders. | `" $icon $available "`
//! `warning` | A value which will trigger warning block state | `20.0`
//! `alert` | A value which will trigger critical block state | `10.0`
//! `info_type` | Determines which information will affect the block state. Possible values are `"available"`, `"free"` and `"used"` | `"available"`
//! `alert_unit` | The unit of `alert` and `warning` options. If not set, percents are uesd. Possible values are `"B"`, `"KB"`, `"MB"`, `"GB"` and `"TB"` | `None`
//!
//! Placeholder  | Value                                                              | Type   | Unit
//! -------------|--------------------------------------------------------------------|--------|-------
//! `icon`       | A static icon                                                      | Icon   | -
//! `path`       | The value of `path` option                                         | Text   | -
//! `percentage` | Free or used percentage. Depends on `info_type`                    | Number | %
//! `total`      | Total disk space                                                   | Number | Bytes
//! `used`       | Used disk space                                                    | Number | Bytes
//! `free`       | Free disk space                                                    | Number | Bytes
//! `available`  | Available disk space (free disk space minus reserved system space) | Number | Bytes
//!
//! # Example
//!
//! ```toml
//! [[block]]
//! block = "disk_space"
//! info_type = "available"
//! alert_unit = "GB"
//! alert = 10.0
//! warning = 15.0
//! format = " $icon $available.eng(2) "
//! ```
//!
//! Update block on right click:
//!
//! ```toml
//! [[block]]
//! block = "disk_space"
//! [[block.click]]
//! button = "right"
//! update = true
//! ```
//!
//! # Icons Used
//! - `disk_drive`

make_log_macro!(debug, "disk_space");

use super::prelude::*;
use crate::formatting::prefix::Prefix;
use nix::sys::statvfs::statvfs;

#[derive(Copy, Clone, Debug, Deserialize, SmartDefault)]
#[serde(rename_all = "lowercase")]
pub enum InfoType {
    #[default]
    Available,
    Free,
    Used,
}

#[derive(Deserialize, Debug, SmartDefault)]
#[serde(deny_unknown_fields, default)]
struct DiskSpaceConfig {
    #[default("/".into())]
    path: ShellString,
    info_type: InfoType,
    format: FormatConfig,
    alert_unit: Option<String>,
    #[default(20.into())]
    interval: Seconds,
    #[default(20.0)]
    warning: f64,
    #[default(10.0)]
    alert: f64,
}

pub async fn run(config: toml::Value, mut api: CommonApi) -> Result<()> {
    let config = DiskSpaceConfig::deserialize(config).config_error()?;
    let mut widget = api
        .new_widget()
        .with_format(config.format.with_default(" $icon $available ")?);

    let unit = match config.alert_unit.as_deref() {
        Some("TB") => Some(Prefix::Tera),
        Some("GB") => Some(Prefix::Giga),
        Some("MB") => Some(Prefix::Mega),
        Some("KB") => Some(Prefix::Kilo),
        Some("B") => Some(Prefix::One),
        Some(x) => return Err(Error::new(format!("Unknown unit: '{x}'"))),
        None => None,
    };

    let path = config.path.expand()?;

    loop {
        let statvfs = statvfs(&*path).error("failed to retrieve statvfs")?;

        let total = (statvfs.blocks() as u64) * (statvfs.fragment_size() as u64);
        let used = ((statvfs.blocks() as u64) - (statvfs.blocks_free() as u64))
            * (statvfs.fragment_size() as u64);
        let available = (statvfs.blocks_available() as u64) * (statvfs.block_size() as u64);
        let free = (statvfs.blocks_free() as u64) * (statvfs.block_size() as u64);

        let result = match config.info_type {
            InfoType::Available => available,
            InfoType::Free => free,
            InfoType::Used => used,
        } as f64;

        let percentage = result / (total as f64) * 100.;
        widget.set_values(map! {
            "icon" => Value::icon(api.get_icon("disk_drive")?),
            "path" => Value::text(path.to_string()),
            "percentage" => Value::percents(percentage),
            "total" => Value::bytes(total as f64),
            "used" => Value::bytes(used as f64),
            "available" => Value::bytes(available as f64),
            "free" => Value::bytes(free as f64),
        });

        // Send percentage to alert check if we don't want absolute alerts
        let alert_val_in_config_units = match unit {
            Some(Prefix::Tera) => result * 1e-12,
            Some(Prefix::Giga) => result * 1e-9,
            Some(Prefix::Mega) => result * 1e-6,
            Some(Prefix::Kilo) => result * 1e-3,
            Some(_) => result,
            None => percentage,
        };

        debug!("alert_val_in_config_units = {alert_val_in_config_units}");

        // Compute state
        widget.state = match config.info_type {
            InfoType::Used => {
                if alert_val_in_config_units >= config.alert {
                    State::Critical
                } else if alert_val_in_config_units >= config.warning {
                    State::Warning
                } else {
                    State::Idle
                }
            }
            InfoType::Free | InfoType::Available => {
                if alert_val_in_config_units <= config.alert {
                    State::Critical
                } else if alert_val_in_config_units <= config.warning {
                    State::Warning
                } else {
                    State::Idle
                }
            }
        };

        api.set_widget(&widget).await?;

        tokio::select! {
            _ = sleep(config.interval.0) => (),
            _ = api.wait_for_update_request() => (),
        }
    }
}

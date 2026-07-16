use crate::config::{BasicLightCommandConfig, CompositeItemConfig, LightCommandConfig};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightStatus {
    Red,
    Green,
    RedFlash,
}

#[derive(Clone)]
pub(crate) struct LightProtocol {
    pub red: Vec<Vec<u8>>,
    pub green: Vec<Vec<u8>>,
    pub red_flash: Vec<RuntimeStep>,
    pub io_timeout: Duration,
}

#[derive(Clone)]
pub(crate) enum RuntimeStep {
    Command(Vec<u8>),
    Repeat {
        repeat: usize,
        steps: Vec<RuntimeRepeatStep>,
    },
}

#[derive(Clone)]
pub(crate) struct RuntimeRepeatStep {
    pub command: Vec<u8>,
    pub delay: Option<Duration>,
}

#[derive(Debug)]
pub(crate) enum LightProtocolError {
    InvalidCommand(String),
    UnknownCommand(String),
}

impl LightProtocol {
    pub(crate) fn from_config(config: &LightCommandConfig) -> Result<Self, LightProtocolError> {
        let basic_commands = parse_basic_commands(&config.basic)?;

        Ok(Self {
            red: resolve_sequence(&config.composite.red, &basic_commands)?,
            green: resolve_sequence(&config.composite.green, &basic_commands)?,
            red_flash: resolve_runtime_steps(&config.composite.red_flash, &basic_commands)?,
            io_timeout: Duration::from_millis(config.timing.io_timeout_ms),
        })
    }
}

pub(crate) fn format_command(command: &[u8]) -> String {
    command
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_basic_commands(
    config: &BasicLightCommandConfig,
) -> Result<HashMap<String, Vec<u8>>, LightProtocolError> {
    let mut commands = HashMap::new();
    commands.insert(
        "green_light_off".to_string(),
        parse_command(&config.green_light_off)?,
    );
    commands.insert(
        "green_light_on".to_string(),
        parse_command(&config.green_light_on)?,
    );
    commands.insert(
        "red_light_on".to_string(),
        parse_command(&config.red_light_on)?,
    );
    commands.insert(
        "red_light_off".to_string(),
        parse_command(&config.red_light_off)?,
    );
    Ok(commands)
}

fn resolve_sequence(
    command_names: &[CompositeItemConfig],
    basic_commands: &HashMap<String, Vec<u8>>,
) -> Result<Vec<Vec<u8>>, LightProtocolError> {
    command_names
        .iter()
        .map(|command_name| match command_name {
            CompositeItemConfig::Command { command } => resolve_command(command, basic_commands),
            CompositeItemConfig::Repeat { .. } => Err(LightProtocolError::InvalidCommand(
                "repeat blocks are not allowed in simple sequences".to_string(),
            )),
        })
        .collect()
}

fn resolve_runtime_steps(
    steps: &[CompositeItemConfig],
    basic_commands: &HashMap<String, Vec<u8>>,
) -> Result<Vec<RuntimeStep>, LightProtocolError> {
    steps
        .iter()
        .map(|step| match step {
            CompositeItemConfig::Command { command } => Ok(RuntimeStep::Command(resolve_command(
                command,
                basic_commands,
            )?)),
            CompositeItemConfig::Repeat { repeat, steps } => Ok(RuntimeStep::Repeat {
                repeat: *repeat,
                steps: steps
                    .iter()
                    .map(|step| {
                        Ok(RuntimeRepeatStep {
                            command: resolve_command(&step.command, basic_commands)?,
                            delay: step.delay_ms.map(Duration::from_millis),
                        })
                    })
                    .collect::<Result<Vec<_>, LightProtocolError>>()?,
            }),
        })
        .collect()
}

fn resolve_command(
    command_name: &str,
    basic_commands: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, LightProtocolError> {
    basic_commands
        .get(command_name)
        .cloned()
        .ok_or_else(|| LightProtocolError::UnknownCommand(command_name.to_string()))
}

fn parse_command(command: &str) -> Result<Vec<u8>, LightProtocolError> {
    let mut bytes = Vec::new();

    for part in command
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == '-')
        .filter(|part| !part.is_empty())
    {
        let hex = part
            .strip_prefix("0x")
            .or_else(|| part.strip_prefix("0X"))
            .unwrap_or(part);

        if hex.len() > 2 {
            if hex.len() % 2 != 0 {
                return Err(LightProtocolError::InvalidCommand(command.to_string()));
            }

            for index in (0..hex.len()).step_by(2) {
                bytes.push(
                    u8::from_str_radix(&hex[index..index + 2], 16)
                        .map_err(|_| LightProtocolError::InvalidCommand(command.to_string()))?,
                );
            }
        } else {
            bytes.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| LightProtocolError::InvalidCommand(command.to_string()))?,
            );
        }
    }

    if bytes.is_empty() {
        return Err(LightProtocolError::InvalidCommand(command.to_string()));
    }

    Ok(bytes)
}

impl std::fmt::Display for LightProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LightProtocolError::InvalidCommand(command) => {
                write!(f, "invalid light command config: {command}")
            }
            LightProtocolError::UnknownCommand(command_name) => {
                write!(f, "unknown light command name: {command_name}")
            }
        }
    }
}

impl std::error::Error for LightProtocolError {}

#[cfg(test)]
mod tests {
    use super::{format_command, parse_command};

    #[test]
    fn formats_command_as_hex_bytes() {
        assert_eq!(
            format_command(&[0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A]),
            "01 05 00 00 FF 00 8C 3A"
        );
    }

    #[test]
    fn parses_configured_command_formats() {
        assert_eq!(
            parse_command("01 05 00 00 FF 00 8C 3A").unwrap(),
            vec![0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A]
        );
        assert_eq!(
            parse_command("01050000FF008C3A").unwrap(),
            vec![0x01, 0x05, 0x00, 0x00, 0xFF, 0x00, 0x8C, 0x3A]
        );
    }
}

use std::{
    fmt::Debug,
    num::{IntErrorKind, ParseIntError},
    str::FromStr,
};

#[derive(Debug, PartialEq, Eq)]
pub enum MediaProp {
    Title(String),
    Connection {
        net_type: NetworkType,
        address_type: AddressType,
        address: String,
        ttl: Option<usize>,
        num_addresses: Option<usize>,
        /// Optional suffix to previous data
        suffix: Option<String>,
    },
    Bandwidth {
        r#type: BandwidthType,
        bandwidth: usize,
    },
    EncryptionKeys(EncryptionKeyMethod),
    Attribute {
        key: String,
        value: Option<String>,
    },
}

impl FromStr for MediaProp {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (key, value) = content_from_line(s)?;
        let tokens = value.split(' ').collect::<Vec<&str>>();

        match key {
            'i' => Ok(MediaProp::Title(value)),
            'c' => {
                let address_split = tokens[2].split('/').collect::<Vec<&str>>();
                let (address, ttl, num_addresses) = get_options_from_address_split(address_split)?;

                let suffix = if tokens.len() > 3 {
                    Some(tokens[3..].join(" "))
                } else {
                    None
                };

                Ok(MediaProp::Connection {
                    net_type: NetworkType::from_str(tokens[0])?,
                    address_type: AddressType::from_str(tokens[1])?,
                    address: address.to_string(),
                    ttl,
                    num_addresses,
                    suffix,
                })
            }
            'b' => {
                let tokens = value.split(':').collect::<Vec<&str>>();

                Ok(MediaProp::Bandwidth {
                    r#type: BandwidthType::from_str(tokens[0])?,
                    bandwidth: tokens[1].parse()?,
                })
            }
            'k' => Ok(MediaProp::EncryptionKeys(EncryptionKeyMethod::from_str(
                &value,
            )?)),
            'a' => {
                let tokens = value.split(':').collect::<Vec<&str>>();

                Ok(if tokens.len() > 1 {
                    MediaProp::Attribute {
                        key: tokens[0].to_string(),
                        value: Some(tokens[1..].join(":")),
                    }
                } else {
                    MediaProp::Attribute {
                        key: value,
                        value: None,
                    }
                })
            }
            _ => Err(ParseError::UnknownToken(s.to_string())),
        }
    }
}

impl ToString for MediaProp {
    fn to_string(&self) -> String {
        match self {
            MediaProp::Title(title) => format!("i={title}"),
            MediaProp::Connection {
                net_type,
                address_type,
                address,
                ttl,
                num_addresses,
                suffix,
            } => {
                // TTL is required for IPv4, but also apparently the major browsers don't like to follow specs so we're gonna ignore that
                let mut address = if let Some(ttl) = ttl {
                    format!("{address}/{}", ttl)
                } else {
                    address.clone()
                };

                if let Some(num_addresses) = num_addresses {
                    address = format!("{address}/{num_addresses}")
                }

                build_connection_lines(net_type, address_type, suffix, &address)
            }
            MediaProp::Bandwidth { r#type, bandwidth } => {
                format!("b={}:{}", r#type.to_string(), bandwidth)
            }
            MediaProp::EncryptionKeys(method) => format!("k={}", method.to_string()),
            MediaProp::Attribute { key, value } => {
                if let Some(value) = value {
                    format!("a={}:{}", key, value)
                } else {
                    format!("a={key}")
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MediaType {
    Audio,
    Video,
    Text,
    Application,
}

impl FromStr for MediaType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "audio" => Ok(MediaType::Audio),
            "video" => Ok(MediaType::Video),
            "text" => Ok(MediaType::Text),
            "application" => Ok(MediaType::Application),
            _ => Err(ParseError::UnknownToken(s.to_string())),
        }
    }
}

impl ToString for MediaType {
    fn to_string(&self) -> String {
        match self {
            MediaType::Audio => "audio",
            MediaType::Video => "video",
            MediaType::Text => "text",
            MediaType::Application => "application",
        }
        .to_string()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NetworkType {
    Internet,
}

impl FromStr for NetworkType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "IN" => Ok(NetworkType::Internet),
            _ => Err(ParseError::UnknownToken(s.to_string())),
        }
    }
}

impl ToString for NetworkType {
    fn to_string(&self) -> String {
        match self {
            NetworkType::Internet => "IN",
        }
        .to_string()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddressType {
    IPv4,
    IPv6,
}

impl FromStr for AddressType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "IP4" => Ok(AddressType::IPv4),
            "IP6" => Ok(AddressType::IPv6),
            _ => Err(ParseError::UnknownToken(s.to_string())),
        }
    }
}

impl ToString for AddressType {
    fn to_string(&self) -> String {
        match self {
            AddressType::IPv4 => "IP4",
            AddressType::IPv6 => "IP6",
        }
        .to_string()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BandwidthType {
    ConferenceTotal,
    ApplicationSpecific,
}

impl FromStr for BandwidthType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CT" => Ok(BandwidthType::ConferenceTotal),
            "AS" => Ok(BandwidthType::ApplicationSpecific),
            _ => Err(ParseError::UnknownToken(s.to_string())),
        }
    }
}

impl ToString for BandwidthType {
    fn to_string(&self) -> String {
        match self {
            BandwidthType::ConferenceTotal => "CT",
            BandwidthType::ApplicationSpecific => "AS",
        }
        .to_string()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TimeZoneAdjustment {
    time: usize,
    offset: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EncryptionKeyMethod {
    Clear(String),
    Base64(String),
    Uri(String),
    Prompt,
}

impl FromStr for EncryptionKeyMethod {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "prompt" {
            return Ok(EncryptionKeyMethod::Prompt);
        }

        let split = s.split(':').collect::<Vec<&str>>();
        let key = split[1..].join(":");
        match split[0] {
            "clear" => Ok(EncryptionKeyMethod::Clear(key)),
            "base64" => Ok(EncryptionKeyMethod::Base64(key)),
            "uri" => Ok(EncryptionKeyMethod::Uri(key)),
            _ => Err(ParseError::UnknownToken(s.to_string())),
        }
    }
}

impl ToString for EncryptionKeyMethod {
    fn to_string(&self) -> String {
        match self {
            EncryptionKeyMethod::Clear(key) => format!("clear:{key}"),
            EncryptionKeyMethod::Base64(key) => format!("base64:{key}"),
            EncryptionKeyMethod::Uri(key) => format!("uri:{key}"),
            EncryptionKeyMethod::Prompt => "prompt".to_string(),
        }
    }
}

// https://datatracker.ietf.org/doc/html/rfc4566#section-2
#[derive(Debug, PartialEq, Eq)]
pub enum SdpProp {
    Version(u8),
    Origin {
        username: String,
        session_id: String,
        session_version: usize,
        net_type: NetworkType,
        address_type: AddressType,
        address: String,
    },
    SessionName(String),
    SessionInformation(String),
    Uri(String),
    Email(String),
    Phone(String),
    Connection {
        net_type: NetworkType,
        address_type: AddressType,
        address: String,
        ttl: Option<usize>,
        num_addresses: Option<usize>,
        /// Optional suffix to previous data
        suffix: Option<String>,
    },
    Bandwidth {
        r#type: BandwidthType,
        bandwidth: usize,
    },
    Timing {
        start: usize,
        stop: usize,
    },
    /// Can be either numbers or numbers with time modifiers (d, h, m, s) so should be strings
    RepeatTimes {
        interval: String,
        active_duration: String,
        start_offsets: Vec<String>,
    },
    TimeZone(Vec<TimeZoneAdjustment>),
    EncryptionKeys(EncryptionKeyMethod),
    Attribute {
        key: String,
        value: Option<String>,
    },
    Media {
        r#type: MediaType,
        ports: Vec<u16>,
        protocol: String,
        format: String,
        props: Vec<MediaProp>,
    },
}

impl FromStr for SdpProp {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (key, value) = content_from_line(s)?;
        let tokens = value.split(' ').collect::<Vec<&str>>();

        // TODO: Cut down on code copying from SDPProp to MediaProp
        match key {
            'v' => Ok(SdpProp::Version(value.parse()?)),
            'o' => Ok(SdpProp::Origin {
                username: tokens[0].to_string(),
                session_id: tokens[1].to_string(),
                session_version: tokens[2].parse()?,
                net_type: NetworkType::from_str(tokens[3])?,
                address_type: AddressType::from_str(tokens[4])?,
                address: tokens[5].to_string(),
            }),
            's' => Ok(SdpProp::SessionName(value)),
            'i' => Ok(SdpProp::SessionInformation(value)),
            'u' => Ok(SdpProp::Uri(value)),
            'e' => Ok(SdpProp::Email(value)),
            'p' => Ok(SdpProp::Phone(value)),
            'c' => {
                let address_split = tokens[2].split('/').collect::<Vec<&str>>();
                let (address, ttl, num_addresses) = get_options_from_address_split(address_split)?;

                let suffix = if tokens.len() > 3 {
                    Some(tokens[3..].join(" "))
                } else {
                    None
                };

                Ok(SdpProp::Connection {
                    net_type: NetworkType::from_str(tokens[0])?,
                    address_type: AddressType::from_str(tokens[1])?,
                    address: address.to_string(),
                    ttl,
                    num_addresses,
                    suffix,
                })
            }
            'b' => {
                let tokens = value.split(':').collect::<Vec<&str>>();

                Ok(SdpProp::Bandwidth {
                    r#type: BandwidthType::from_str(tokens[0])?,
                    bandwidth: tokens[1].parse()?,
                })
            }
            't' => Ok(SdpProp::Timing {
                start: tokens[0].parse()?,
                stop: tokens[1].parse()?,
            }),
            'r' => Ok(SdpProp::RepeatTimes {
                interval: tokens[0].to_string(),
                active_duration: tokens[1].to_string(),
                start_offsets: tokens[2..]
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<String>>(),
            }),
            'z' => {
                let mut adjustments = Vec::new();
                for group in tokens.chunks(2) {
                    adjustments.push(TimeZoneAdjustment {
                        time: group[0].to_string().parse()?,
                        offset: group[1].to_string(),
                    });
                }

                Ok(SdpProp::TimeZone(adjustments))
            }
            'k' => Ok(SdpProp::EncryptionKeys(EncryptionKeyMethod::from_str(
                &value,
            )?)),
            'a' => {
                let tokens = value.split(':').collect::<Vec<&str>>();

                Ok(if tokens.len() > 1 {
                    SdpProp::Attribute {
                        key: tokens[0].to_string(),
                        value: Some(tokens[1..].join(":")),
                    }
                } else {
                    SdpProp::Attribute {
                        key: value,
                        value: None,
                    }
                })
            }
            // Media lines can have their own attributes, the entire media block will be passed in
            'm' => {
                let lines = value.split('\n').collect::<Vec<&str>>();
                let tokens = lines[0].split(' ').collect::<Vec<&str>>();

                Ok(SdpProp::Media {
                    r#type: MediaType::from_str(tokens[0])?,
                    ports: tokens[1]
                        .split('/')
                        .map(|port| port.parse())
                        .collect::<Result<Vec<_>, _>>()?,
                    protocol: tokens[2].to_string(),
                    format: tokens[3..].join(" "),
                    props: lines[1..]
                        .iter()
                        .map(|line| MediaProp::from_str(line))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            _ => Err(ParseError::UnknownKey(key, value)),
        }
    }
}

impl ToString for SdpProp {
    fn to_string(&self) -> String {
        // TODO: Cut down on code copying from SDPProp to MediaProp
        match self {
            SdpProp::Version(v) => format!("v={v}"),
            SdpProp::Origin {
                username,
                session_id,
                session_version,
                net_type,
                address_type,
                address,
            } => format!(
                "o={username} {session_id} {session_version} {} {} {address}",
                net_type.to_string(),
                address_type.to_string()
            ),
            SdpProp::SessionName(name) => format!("s={name}"),
            SdpProp::SessionInformation(info) => format!("i={info}"),
            SdpProp::Uri(uri) => format!("u={uri}"),
            SdpProp::Email(email) => format!("e={email}"),
            SdpProp::Phone(phone) => format!("p={phone}"),
            SdpProp::Connection {
                net_type,
                address_type,
                address,
                ttl,
                num_addresses,
                suffix,
            } => {
                // TTL is required for IPv4
                let mut address = if *address_type == AddressType::IPv4 || ttl.is_some() {
                    format!("{address}/{}", ttl.unwrap())
                } else {
                    address.clone()
                };

                if let Some(num_addresses) = num_addresses {
                    address = format!("{address}/{num_addresses}")
                }

                build_connection_lines(net_type, address_type, suffix, &address)
            }
            SdpProp::Bandwidth { r#type, bandwidth } => {
                format!("b={}:{}", r#type.to_string(), bandwidth)
            }
            SdpProp::Timing { start, stop } => format!("t={start} {stop}"),
            SdpProp::RepeatTimes {
                interval,
                active_duration,
                start_offsets,
            } => format!("r={interval} {active_duration} {}", start_offsets.join(" ")),
            SdpProp::TimeZone(adjustments) => format!(
                "z={}",
                adjustments
                    .iter()
                    .map(|adj| format!("{} {}", adj.time, adj.offset))
                    .collect::<Vec<String>>()
                    .join(" ")
            ),
            SdpProp::EncryptionKeys(method) => format!("k={}", method.to_string()),
            SdpProp::Attribute { key, value } => {
                if let Some(value) = value {
                    format!("a={}:{}", key, value)
                } else {
                    format!("a={key}")
                }
            }
            SdpProp::Media {
                r#type,
                ports,
                protocol,
                format,
                props,
            } => {
                let header = format!(
                    "{} {} {} {}",
                    r#type.to_string(),
                    ports
                        .iter()
                        .map(|port| port.to_string())
                        .collect::<Vec<String>>()
                        .join("/"),
                    protocol.to_string(),
                    format
                );

                format!(
                    "m={header}{}",
                    if props.is_empty() {
                        "".to_string()
                    } else {
                        format!(
                            "\r\n{}",
                            props
                                .iter()
                                .map(|prop| prop.to_string())
                                .collect::<Vec<String>>()
                                .join("\r\n")
                        )
                    }
                )
            }
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    /// Unknown attribute key along with its value
    UnknownKey(char, String),
    UnknownToken(String),
    /// Failed to cast from String to another type
    TypeParseFailed(IntErrorKind),
}

impl From<ParseIntError> for ParseError {
    fn from(e: ParseIntError) -> Self {
        Self::TypeParseFailed(e.kind().clone())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SDP {
    pub props: Vec<SdpProp>,
}

impl FromStr for SDP {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Convert \r\n to \n
        let s = s.replace("\r\n", "\n");

        // Split string
        let lines = s
            .split('\n')
            .map(|line| line.to_string())
            .collect::<Vec<String>>();

        // Group media attributes
        // Find indexes of all media lines
        let m_indices = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with('m'))
            .map(|(idx, _)| idx)
            .collect::<Vec<_>>();

        // Combine all media sections into one line per section
        let lines: Vec<String> =
            lines
                .into_iter()
                .filter(|line| !line.is_empty())
                .enumerate()
                .fold(Vec::new(), |mut acc, (idx, line)| {
                    // If m-line detected or array empty, start a new section
                    if acc.is_empty()
                        || m_indices.contains(&idx)
                        || m_indices.is_empty()
                        || idx < m_indices[0]
                    {
                        acc.push(line);
                        return acc;
                    }

                    // Add to current section
                    *acc.last_mut().unwrap() = format!("{}\n{line}", acc.last_mut().unwrap());

                    acc
                });

        Ok(Self {
            props: lines
                .into_iter()
                .map(|line| SdpProp::from_str(&line))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl ToString for SDP {
    fn to_string(&self) -> String {
        format!("{}\r\n", self.props
            .iter()
            .map(|prop| prop.to_string())
            .collect::<Vec<String>>()
            .join("\r\n"))
    }
}

fn content_from_line(line: &str) -> Result<(char, String), ParseError> {
    let split = line.split('=').collect::<Vec<&str>>();
    if split.len() < 2 {
        return Err(ParseError::UnknownToken(line.to_string()));
    }
    Ok((split[0].chars().next().unwrap(), split[1..].join("=")))
}

fn get_options_from_address_split(address_split: Vec<&str>) -> Result<(&str, Option<usize>, Option<usize>), ParseError> {
    Ok(match address_split.len() {
        1 => (address_split[0], None, None),
        2 => (address_split[0], Some(address_split[1].parse()?), None),
        3 => (
            address_split[0],
            Some(address_split[1].parse()?),
            Some(address_split[2].parse()?),
        ),
        _ => unreachable!(),
    })
}

fn build_connection_lines(net_type: &NetworkType, address_type: &AddressType, suffix: &Option<String>, address: &String) -> String {
    if let Some(suffix) = suffix {
        format!(
            "c={} {} {} {}",
            net_type.to_string(),
            address_type.to_string(),
            address,
            suffix
        )
    } else {
        format!(
            "c={} {} {}",
            net_type.to_string(),
            address_type.to_string(),
            address
        )
    }
}
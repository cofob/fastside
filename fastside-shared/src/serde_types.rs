use std::{collections::HashMap, fmt, vec};

use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use url::Url;

#[derive(Deserialize, Serialize, Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instance {
    pub url: Url,
    pub tags: Vec<String>,
}

impl From<Url> for Instance {
    fn from(url: Url) -> Self {
        Instance {
            url,
            tags: Vec::new(),
        }
    }
}

fn default_test_url() -> String {
    "/".to_string()
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegexSearch {
    pub regex: String,
    pub url: String,
}

pub trait HttpCodeRanges {
    fn is_allowed(&self, code: u16) -> bool;
}

#[derive(Debug, Clone)]
pub struct AllowedHttpCodes {
    pub codes: Vec<u16>,
    pub inclusive_ranges: Vec<(u16, u16)>,
    pub exclusive_ranges: Vec<(u16, u16)>,
}

impl HttpCodeRanges for AllowedHttpCodes {
    fn is_allowed(&self, code: u16) -> bool {
        if self.codes.contains(&code) {
            return true;
        }

        for &(start, end) in &self.inclusive_ranges {
            if code >= start && code <= end {
                return true;
            }
        }

        for &(start, end) in &self.exclusive_ranges {
            if code >= start && code < end {
                return true;
            }
        }

        false
    }
}

impl<'de> Deserialize<'de> for AllowedHttpCodes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AllowedHttpCodesVisitor;

        impl<'de> Visitor<'de> for AllowedHttpCodesVisitor {
            type Value = AllowedHttpCodes;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string representing allowed HTTP codes and ranges")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let mut codes = Vec::new();
                let mut inclusive_ranges = Vec::new();
                let mut exclusive_ranges = Vec::new();

                for part in value.split(',') {
                    if part.contains("..=") {
                        let mut split = part.split("..=");
                        let start = split.next().unwrap().trim();
                        let end = split.next().unwrap().trim();
                        let start = start.parse::<u16>().map_err(de::Error::custom)?;
                        let end = end.parse::<u16>().map_err(de::Error::custom)?;
                        inclusive_ranges.push((start, end));
                    } else if part.contains("..") {
                        let mut split = part.split("..");
                        let start = split.next().unwrap().trim();
                        let end = split.next().unwrap().trim();
                        let start = start.parse::<u16>().map_err(de::Error::custom)?;
                        let end = end.parse::<u16>().map_err(de::Error::custom)?;
                        exclusive_ranges.push((start, end));
                    } else {
                        let code = part.trim().parse::<u16>().map_err(de::Error::custom)?;
                        codes.push(code);
                    }
                }

                Ok(AllowedHttpCodes {
                    codes,
                    inclusive_ranges,
                    exclusive_ranges,
                })
            }
        }

        deserializer.deserialize_str(AllowedHttpCodesVisitor)
    }
}

impl Serialize for AllowedHttpCodes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut parts = Vec::new();

        for &code in &self.codes {
            parts.push(code.to_string());
        }

        for &(start, end) in &self.inclusive_ranges {
            parts.push(format!("{}..={}", start, end));
        }

        for &(start, end) in &self.exclusive_ranges {
            parts.push(format!("{}..{}", start, end));
        }

        let result = parts.join(",");
        serializer.serialize_str(&result)
    }
}

fn default_allowed_http_codes() -> AllowedHttpCodes {
    AllowedHttpCodes {
        codes: vec![200],
        inclusive_ranges: Vec::new(),
        exclusive_ranges: Vec::new(),
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Service {
    #[serde(rename = "type")]
    pub name: String,
    #[serde(default = "default_test_url")]
    pub test_url: String,
    #[serde(default)]
    pub fallback: Option<Url>,
    #[serde(default)]
    pub follow_redirects: bool,
    #[serde(default = "default_allowed_http_codes")]
    pub allowed_http_codes: AllowedHttpCodes,
    #[serde(default)]
    pub search_string: Option<String>,
    #[serde(default)]
    pub regexes: Vec<RegexSearch>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub source_link: Option<Url>,
    #[serde(default)]
    pub deprecated_message: Option<String>,
    pub instances: Vec<Instance>,
}

pub type ServicesData = HashMap<String, Service>;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct StoredData {
    pub services: Vec<Service>,
}

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

pub struct ValidationResults {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub notices: Vec<String>,
}

impl ValidationResults {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
            notices: Vec::new(),
        }
    }

    pub fn add_error(&mut self, error: String) {
        self.errors.push(error);
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub fn add_notice(&mut self, notice: String) {
        self.notices.push(notice);
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty() && self.notices.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn format(&self) -> String {
        let mut result = String::new();

        for error in &self.errors {
            result.push_str(&format!("Error: {}\n", error));
        }

        for warning in &self.warnings {
            result.push_str(&format!("Warning: {}\n", warning));
        }

        for notice in &self.notices {
            result.push_str(&format!("Notice: {}\n", notice));
        }

        result
    }
}

impl Default for ValidationResults {
    fn default() -> Self {
        Self::new()
    }
}

impl StoredData {
    pub fn validate(&self) -> ValidationResults {
        let mut results = ValidationResults::new();

        // Check if all regexes are valid
        {
            for service in &self.services {
                for regex in &service.regexes {
                    regex::Regex::new(&regex.regex)
                        .map_err(|e| {
                            results.add_error(format!(
                                "Service {} has invalid regex {}: {}",
                                service.name, regex.regex, e
                            ));
                            e
                        })
                        .ok();
                }
            }
        }

        // Check if service has no instances and no deprecation message
        {
            for service in &self.services {
                if service.instances.is_empty() && service.deprecated_message.is_none() {
                    results.add_warning(format!(
                        "Service {} has no instances and no deprecation message",
                        service.name
                    ));
                }
            }
        }

        // Check if all instance URLs are unique
        {
            let mut instance_urls = HashMap::new();

            for service in &self.services {
                for instance in &service.instances {
                    if instance_urls.contains_key(&instance.url) {
                        results.add_warning(format!(
                            "Service {} has duplicate instance URL {}",
                            service.name, instance.url
                        ));
                    } else {
                        instance_urls.insert(&instance.url, &service.name);
                    }
                }
            }
        }

        // Check if all aliases are unique
        {
            let mut aliases = HashMap::new();

            for service in &self.services {
                for alias in &service.aliases {
                    if aliases.contains_key(alias) {
                        results.add_warning(format!(
                            "Service {} has duplicate alias {}",
                            service.name, alias
                        ));
                    } else {
                        aliases.insert(alias, &service.name);
                    }
                }
            }
        }

        // Check if all names and aliases in correct format
        {
            let name_regex = regex::Regex::new(r"^[a-z0-9-]+$").unwrap();

            for service in &self.services {
                if !name_regex.is_match(&service.name) {
                    results
                        .add_warning(format!("Service {} has invalid name format", service.name));
                }

                for alias in &service.aliases {
                    if !name_regex.is_match(alias) {
                        results.add_warning(format!(
                            "Service {} has invalid alias format {}",
                            service.name, alias
                        ));
                    }
                }
            }
        }

        // Check if all URLs have host
        {
            for service in &self.services {
                if let Some(fallback) = &service.fallback {
                    if fallback.host_str().is_none() {
                        results.add_warning(format!(
                            "Service {} has fallback URL without host",
                            service.name
                        ));
                    }
                }

                for instance in &service.instances {
                    if instance.url.host_str().is_none() {
                        results.add_warning(format!(
                            "Service {} has instance URL without host",
                            service.name
                        ));
                    }
                }
            }
        }

        results
    }
}

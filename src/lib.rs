extern crate chrono;
extern crate deb_architectures;
#[macro_use]
extern crate smart_default;

use chrono::{DateTime, Utc};
use deb_architectures::Architecture;
use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::{fs, io};

/// The dist release file is a file in the apt repository that points to all other dist files in the archive.
#[derive(Debug, SmartDefault, Clone, PartialEq)]
pub struct DistRelease {
    pub architectures: Vec<String>,
    pub codename: String,
    pub components: Vec<String>,
    #[default = "Utc::now()"]
    pub date: DateTime<Utc>,
    pub description: String,
    pub label: String,
    pub origin: String,
    pub suite: String,
    pub version: String,
    pub sums: BTreeMap<String, EntryComponents>,
}

impl DistRelease {
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        fs::read_to_string(path).and_then(|string| string.parse::<Self>())
    }
}

impl FromStr for DistRelease {
    type Err = io::Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut iterator = input.lines();

        let mut release = DistRelease::default();

        #[derive(Copy, Clone)]
        enum Variant {
            Archs,
            Codename,
            Components,
            Date,
            Description,
            Label,
            Origin,
            Suite,
            Version,
        }

        let mut entries = vec![
            ("Architectures:", Variant::Archs),
            ("Codename:", Variant::Codename),
            ("Components:", Variant::Components),
            ("Date:", Variant::Date),
            ("Description:", Variant::Description),
            ("Label:", Variant::Label),
            ("Origin:", Variant::Origin),
            ("Suite:", Variant::Suite),
            ("Version:", Variant::Version),
        ];

        let mut remove = None;

        fn get_string(value: &str) -> String {
            value.trim().to_owned()
        }

        fn get_vec(value: &str) -> Vec<String> {
            value.split_whitespace().map(String::from).collect()
        }

        while !entries.is_empty() {
            let line = iterator.next().unwrap();

            for (id, &(ref key, variant)) in entries.iter().enumerate() {
                if line.starts_with(key) {
                    remove = Some(id);

                    let value = &line[key.len()..];

                    match variant {
                        Variant::Archs => release.architectures = get_vec(value),
                        Variant::Codename => release.codename = get_string(value),
                        Variant::Components => release.components = get_vec(value),
                        Variant::Date => release.date = get_time(value)?,
                        Variant::Description => release.description = get_string(value),
                        Variant::Label => release.label = get_string(value),
                        Variant::Origin => release.origin = get_string(value),
                        Variant::Suite => release.suite = get_string(value),
                        Variant::Version => release.version = get_string(value),
                    }
                }
            }

            if let Some(pos) = remove.take() {
                entries.remove(pos);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown key in release file: {}", line),
                ));
            }
        }

        let mut active_hash = String::new();
        let mut active_components = EntryComponents::default();

        for line in iterator {
            if line.starts_with(' ') {
                match line.parse::<ReleaseEntry>() {
                    Ok(entry) => {
                        let base = match entry.path.rfind('.') {
                            Some(pos) => &entry.path[..pos],
                            None => &entry.path,
                        };

                        match entry.path.find('/') {
                            Some(pos) => {
                                let component = &entry.path[..pos];
                                active_components.components
                                    .entry(component.into())
                                    .and_modify(|e| {
                                        e.entry(base.into())
                                            .and_modify(|e| e.push(entry.to_owned()))
                                            .or_insert_with(|| vec![entry.to_owned()]);
                                    })
                                    .or_insert_with(|| {
                                        let mut map = BTreeMap::new();
                                        map.insert(base.into(), vec![entry.to_owned()]);
                                        map
                                    });
                            }
                            None => {
                                active_components.base
                                    .entry(base.into())
                                    .and_modify(|e| e.push(entry.to_owned()))
                                    .or_insert_with(|| vec![entry.to_owned()]);
                            }
                        }
                    }
                    Err(why) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("invalid checksum entry: {}", why),
                        ))
                    }
                }
            } else {
                if !active_components.is_empty() {
                    release
                        .sums
                        .insert(active_hash.clone(), active_components.clone());
                    active_components.clear();
                }

                active_hash.clear();
                active_hash.push_str(line.trim());
                active_hash.pop();
            }
        }

        if !active_components.is_empty() {
            release
                .sums
                .insert(active_hash, active_components);
        }

        Ok(release)
    }
}

/// Stores the entries for each component for this checksum method.
#[derive(Debug, Default, Clone, Hash, PartialEq)]
pub struct EntryComponents {
    pub base: BTreeMap<String, Vec<ReleaseEntry>>,
    pub components: BTreeMap<String, BTreeMap<String, Vec<ReleaseEntry>>>,
}

impl EntryComponents {
    pub fn clear(&mut self) {
        self.base.clear();
        self.components.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.base.is_empty() && self.components.is_empty()
    }
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub enum ReleaseVariant {
    Binary(Architecture),
    Contents(Architecture),
    Components(Architecture),
    Source,
    Translation(String),
}

/// The hash, size, and path of a file that this release file points to.
#[derive(Debug, Default, Clone, Hash, PartialEq)]
pub struct ReleaseEntry {
    pub sum: String,
    pub size: u64,
    pub path: String,
}

impl ReleaseEntry {
    pub fn variant(&self) -> Option<ReleaseVariant> {
        let mut components = self.path.split('/');

        while let Some(component) = components.next() {
            if component == "source" {
                return Some(ReleaseVariant::Source);
            } else if component == "i18n" {
                while let Some(component) = components.next() {
                    if let Some(lang) = component.split('-').nth(1) {
                        let lang = match lang.find('.') {
                            Some(pos) => &lang[..pos],
                            None => lang,
                        };

                        return Some(ReleaseVariant::Translation(lang.to_owned()));
                    }
                }

                break;
            }

            macro_rules! fetch_arch {
                ($kind:tt) => {
                    fetch_arch!($kind => $kind);
                };
                ($kind:tt => $variant:tt) => {
                    if component.starts_with(stringify!($kind)) {
                        if let Some(arch) = component.split('-').nth(1) {
                            let arch = match arch.find('.') {
                                Some(pos) => &arch[..pos],
                                None => arch,
                            };

                            if let Ok(arch) = arch.parse::<Architecture>() {
                                return Some(ReleaseVariant::$variant(arch));
                            }
                        }
                    }
                };
            }

            fetch_arch!(binary => Binary);
            fetch_arch!(Contents);
            fetch_arch!(Components);
        }

        None
    }
}

impl FromStr for ReleaseEntry {
    type Err = &'static str;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut iterator = input.split_whitespace();

        let output = Self {
            sum: iterator.next().ok_or("missing sum field")?.to_owned(),
            size: iterator
                .next()
                .ok_or("missing size field")?
                .parse::<u64>()
                .map_err(|_| "size field is not a number")?,
            path: iterator.next().ok_or("missing path field")?.to_owned(),
        };

        Ok(output)
    }
}

fn get_time(value: &str) -> io::Result<DateTime<Utc>> {
    let fields = value.split_whitespace().collect::<Vec<&str>>();
    if fields.len() != 6 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "timezone is invalid: should have been six fields: {}",
                value
            ),
        ));
    }

    let mut buffer = fields[..4].join(" ");
    buffer.push(' ');

    if fields[4].split(':').next().unwrap().len() == 1 {
        buffer.push('0');
    }

    buffer.push_str(&fields[4]);
    buffer.push(' ');

    if fields[5] == "UTC" {
        buffer.push_str("+0000");
    } else {
        buffer.push_str(&fields[5])
    };

    DateTime::parse_from_rfc2822(&buffer)
        .map(|tz| tz.with_timezone(&Utc))
        .map_err(|why| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unable to parse date ({}) in release file: {}", buffer, why),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debian_rfc2822_quirks() {
        let time = "Wed, 28 Nov 2018 3:16:40 UTC";
        assert!(get_time(time).is_ok());
    }
}

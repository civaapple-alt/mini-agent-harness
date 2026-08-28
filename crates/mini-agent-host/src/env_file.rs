use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Default)]
pub struct Environment {
    file_values: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueSource {
    Process,
    EnvFile,
    #[allow(dead_code)]
    UserEnv,
}

#[derive(Clone)]
pub struct ResolvedValue {
    pub value: String,
    pub source: ValueSource,
}

impl Environment {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        parse(&source).map(|file_values| Self { file_values })
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.file_values
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    pub fn resolve(&self, name: &str) -> Option<ResolvedValue> {
        env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| ResolvedValue {
                value,
                source: ValueSource::Process,
            })
            .or_else(|| {
                self.get(name).map(|value| ResolvedValue {
                    value: value.to_string(),
                    source: ValueSource::EnvFile,
                })
            })
    }
}

fn parse(source: &str) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, raw_value) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid .env line {line_number}: expected NAME=VALUE"))?;
        let name = name.trim();
        if !valid_name(name) {
            return Err(format!("invalid .env name on line {line_number}: {name}"));
        }
        let value = unquote(raw_value.trim(), line_number)?;
        values.insert(name.to_string(), value.to_string());
    }
    Ok(values)
}

fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn unquote(value: &str, line_number: usize) -> Result<&str, String> {
    let quoted = value.starts_with(['\'', '"']) || value.ends_with(['\'', '"']);
    if !quoted {
        return Ok(value);
    }
    if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        Ok(&value[1..value.len() - 1])
    } else {
        Err(format!("unmatched quote on .env line {line_number}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_empty_values_and_quotes() {
        let values = parse(
            "# local config\nOPENAI_API_KEY=\nOPENAI_MODEL='test-model'\nOPENAI_BASE_URL=\"http://localhost:8080/v1\"\n",
        )
        .unwrap();

        assert_eq!(values["OPENAI_API_KEY"], "");
        assert_eq!(values["OPENAI_MODEL"], "test-model");
        assert_eq!(values["OPENAI_BASE_URL"], "http://localhost:8080/v1");
    }

    #[test]
    fn rejects_invalid_lines() {
        assert_eq!(
            parse("NOT VALID").unwrap_err(),
            "invalid .env line 1: expected NAME=VALUE"
        );
        assert_eq!(
            parse("1INVALID=value").unwrap_err(),
            "invalid .env name on line 1: 1INVALID"
        );
    }
}

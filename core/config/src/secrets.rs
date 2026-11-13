//! Secrets live in Windows Credential Manager (via `keyring`). The config file only holds a
//! reference such as `credman:op-workflow/openproject`; the value never touches disk or logs.

/// Credential Manager service name used for every op-workflow secret.
pub const SERVICE: &str = "op-workflow";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("invalid secret reference `{0}` (expected credman:<service>/<name>)")]
    BadReference(String),
    #[error("credential store: {0}")]
    Store(String),
}

pub type SecretResult<T> = Result<T, SecretError>;

/// `credman:<service>/<name>` → `(service, name)`.
pub fn parse_reference(reference: &str) -> SecretResult<(&str, &str)> {
    let bad = || SecretError::BadReference(reference.to_owned());
    let rest = reference.strip_prefix("credman:").ok_or_else(bad)?;
    let (service, name) = rest.split_once('/').ok_or_else(bad)?;
    if service.is_empty() || name.is_empty() || name.contains('/') {
        return Err(bad());
    }
    Ok((service, name))
}

fn entry(reference: &str) -> SecretResult<keyring::Entry> {
    let (service, name) = parse_reference(reference)?;
    keyring::Entry::new(service, name).map_err(|e| SecretError::Store(e.to_string()))
}

/// The stored value, or `None` when nothing is saved under this reference.
pub fn get(reference: &str) -> SecretResult<Option<String>> {
    match entry(reference)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretError::Store(e.to_string())),
    }
}

pub fn set(reference: &str, value: &str) -> SecretResult<()> {
    entry(reference)?
        .set_password(value)
        .map_err(|e| SecretError::Store(e.to_string()))
}

/// Remove the value; succeeds when there was nothing to remove.
pub fn delete(reference: &str) -> SecretResult<()> {
    match entry(reference)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::Store(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credman_references() {
        assert_eq!(
            parse_reference("credman:op-workflow/openproject").unwrap(),
            ("op-workflow", "openproject")
        );
        for bad in [
            "op-workflow/openproject",
            "credman:",
            "credman:x",
            "credman:/n",
            "credman:s/",
            "credman:s/a/b",
        ] {
            assert!(parse_reference(bad).is_err(), "{bad}");
        }
    }
}

//! Process role definitions and parsing.

use std::fmt;
use std::str::FromStr;

use crate::error::AppError;

/// Runtime roles that may be selected via `run --roles`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Ingress,
    Worker,
    Scheduler,
    Maintenance,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ingress => "ingress",
            Self::Worker => "worker",
            Self::Scheduler => "scheduler",
            Self::Maintenance => "maintenance",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ingress" => Ok(Self::Ingress),
            "worker" => Ok(Self::Worker),
            "scheduler" => Ok(Self::Scheduler),
            "maintenance" => Ok(Self::Maintenance),
            "all" => Err(AppError::usage(
                "use --roles without 'all' or omit for all roles",
            )),
            _ => Err(AppError::usage(format!("unknown role: {}", s))),
        }
    }
}

/// All roles for the default all-in-one profile.
pub fn all_roles() -> Vec<Role> {
    vec![
        Role::Ingress,
        Role::Worker,
        Role::Scheduler,
        Role::Maintenance,
    ]
}

/// Parse comma-separated role list. Empty means all roles.
pub fn parse_roles(values: &[String]) -> Result<Vec<Role>, AppError> {
    if values.is_empty() {
        return Ok(all_roles());
    }

    let mut roles = Vec::new();
    for value in values {
        if value == "all" {
            return Ok(all_roles());
        }
        let role = Role::from_str(value)?;
        if !roles.contains(&role) {
            roles.push(role);
        }
    }
    Ok(roles)
}

/// Options for the supervised runtime.
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub roles: Vec<Role>,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self { roles: all_roles() }
    }
}

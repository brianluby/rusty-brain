use types::RustyBrainError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HookError {
    #[error("[E_HOOK_IO] {message}")]
    Io {
        message: String,
        #[source]
        source: Option<std::io::Error>,
    },

    #[error("[E_HOOK_PARSE] {message}")]
    Parse { message: String },

    #[error("[E_HOOK_MIND] {message}")]
    Mind {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("[E_HOOK_PLATFORM] {message}")]
    Platform { message: String },

    #[error("[E_HOOK_GIT] {message}")]
    Git { message: String },

    #[error("[E_HOOK_DEDUP] {message}")]
    Dedup { message: String },
}

impl From<std::io::Error> for HookError {
    fn from(err: std::io::Error) -> Self {
        Self::Io {
            message: err.to_string(),
            source: Some(err),
        }
    }
}

impl From<serde_json::Error> for HookError {
    fn from(err: serde_json::Error) -> Self {
        Self::Parse {
            message: err.to_string(),
        }
    }
}

impl From<RustyBrainError> for HookError {
    fn from(err: RustyBrainError) -> Self {
        Self::Mind {
            message: err.to_string(),
            source: Some(Box::new(err)),
        }
    }
}

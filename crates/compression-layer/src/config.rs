/// Which agent's tool-name conventions to use when dispatching compressors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    /// Claude Code — tool names: `Bash`, `Read`, `Grep`, `Glob`
    Claude,
    /// Codex CLI — tool names: `shell_command`, `read_file`, `grep`, `list_directory`
    Codex,
    /// OpenCode — tool names: `bash`, `read`, `grep`, `glob`
    OpenCode,
}

/// Configuration for the compression layer.
#[derive(Debug, Clone, bon::Builder)]
pub struct CompressionConfig {
    pub agent: AgentType,
}

const _: () = {
    use compression_config_builder::*;

    impl CompressionConfig {
        pub fn claude() -> CompressionConfigBuilder<SetAgent> {
            Self::builder().claude()
        }

        pub fn codex() -> CompressionConfigBuilder<SetAgent> {
            Self::builder().codex()
        }

        pub fn opencode() -> CompressionConfigBuilder<SetAgent> {
            Self::builder().opencode()
        }
    }

    impl<S: State> CompressionConfigBuilder<S> {
        pub fn claude(self) -> CompressionConfigBuilder<SetAgent<S>>
        where
            S::Agent: IsUnset,
        {
            self.agent(AgentType::Claude)
        }

        pub fn codex(self) -> CompressionConfigBuilder<SetAgent<S>>
        where
            S::Agent: IsUnset,
        {
            self.agent(AgentType::Codex)
        }

        pub fn opencode(self) -> CompressionConfigBuilder<SetAgent<S>>
        where
            S::Agent: IsUnset,
        {
            self.agent(AgentType::OpenCode)
        }
    }
};

import {
  AgentConfig,
  ClaudeProviderConfig,
  CodexProviderConfig,
  GeminiProviderConfig,
  OpenCodeProviderConfig,
  ProviderConfig,
  ProviderName,
} from "../../types";

function isKnownProviderName(provider: string | undefined): provider is ProviderName {
  return provider === "claude" || provider === "codex" || provider === "gemini" || provider === "opencode" || provider === "mock";
}

function providerValue(provider: AgentConfig["provider"]): string {
  return provider && provider.trim() ? provider : "claude";
}

export function normalizeProviderName(provider: AgentConfig["provider"]): ProviderName {
  if (isKnownProviderName(provider)) {
    return provider;
  }
  return "claude";
}

export function defaultProviderConfig(provider: AgentConfig["provider"]): ProviderConfig {
  switch (normalizeProviderName(provider)) {
    case "codex":
      return { type: "codex" };
    case "gemini":
      return { type: "gemini" };
    case "opencode":
      return { type: "opencode" };
    case "mock":
      return { type: "mock" };
    case "claude":
    default:
      return { type: "claude" };
  }
}

function providerConfigMatches(
  provider: ProviderName,
  config: AgentConfig["provider_config"],
): config is ProviderConfig {
  return Boolean(config && config.type === provider);
}

export function providerConfigFor(config: Partial<AgentConfig>, provider = config.provider): ProviderConfig {
  const providerName = normalizeProviderName(provider);
  if (providerConfigMatches(providerName, config.provider_config)) {
    return config.provider_config as ProviderConfig;
  }
  return defaultProviderConfig(providerName);
}

function legacyProviderConfig(config: AgentConfig, provider: ProviderName): ProviderConfig {
  switch (provider) {
    case "codex": {
      const codex: CodexProviderConfig = {
        type: "codex",
        sandbox_mode: config.codex_sandbox_mode,
        approval_policy: config.codex_approval_policy,
        profile: config.codex_profile,
        full_auto: config.codex_full_auto,
        search: config.codex_search,
        skip_git_repo_check: config.codex_skip_git_repo_check,
        ephemeral: config.codex_ephemeral,
        cleared_provider_sessions: config.codex_cleared_provider_sessions,
      };
      return stripUndefined(codex);
    }
    case "gemini": {
      const gemini: GeminiProviderConfig = {
        type: "gemini",
        sandbox: config.sandbox,
        yolo: config.yolo,
        approval_mode: config.approval_mode,
        policy: config.policy,
        experimental_acp: config.experimental_acp,
        allowed_mcp_server_names: config.allowed_mcp_server_names,
        extensions: config.extensions,
        screen_reader: config.screen_reader,
        output_format: config.output_format,
      };
      return stripUndefined(gemini);
    }
    case "opencode": {
      const opencode: OpenCodeProviderConfig = {
        type: "opencode",
        agent: config.opencode_agent,
        port: config.opencode_port,
      };
      return stripUndefined(opencode);
    }
    case "mock":
      return { type: "mock" };
    case "claude":
    default: {
      const claude: ClaudeProviderConfig = {
        type: "claude",
        permission_mode: config.permission_mode,
        max_turns: config.max_turns,
        allowed_tools: config.allowed_tools,
        disallowed_tools: config.disallowed_tools,
        append_system_prompt: config.append_system_prompt,
        mcp_config: config.mcp_config,
      };
      return stripUndefined(claude);
    }
  }
}

function stripUndefined<T extends object>(value: T): T {
  return Object.fromEntries(
    Object.entries(value).filter(([, entry]) => entry !== undefined),
  ) as T;
}

export function normalizeAgentConfig(config: AgentConfig): AgentConfig {
  const provider = providerValue(config.provider);
  if (!isKnownProviderName(provider)) {
    return {
      ...config,
      provider,
      provider_config: config.provider_config ?? { type: provider },
    };
  }

  const provider_config = providerConfigMatches(provider, config.provider_config)
    ? config.provider_config
    : legacyProviderConfig(config, provider);
  return {
    ...config,
    provider,
    provider_config,
  };
}

export function normalizeAgentConfigs(configs: AgentConfig[]): AgentConfig[] {
  return configs.map(normalizeAgentConfig);
}

export function toPersistedAgentConfig(config: AgentConfig): AgentConfig {
  const normalized = normalizeAgentConfig(config);
  if (!isKnownProviderName(normalized.provider)) {
    return normalized;
  }

  return {
    ...normalized,
    provider_config: providerConfigFor(normalized),
  };
}

export function withProvider(
  config: Partial<AgentConfig>,
  provider: ProviderName,
  options: { preserveCustomArgs?: boolean } = {},
): Partial<AgentConfig> {
  return {
    ...config,
    provider,
    provider_config: defaultProviderConfig(provider),
    custom_args: options.preserveCustomArgs ? config.custom_args : undefined,
  };
}

/**
 * Checks if the changes between two AgentConfigs require an agent restart.
 * Restarts are required if any field OTHER than session_name or session_id is changed.
 */
export function requiresRestart(oldConfig: AgentConfig, newConfig: AgentConfig): boolean {
  const keys = Object.keys(newConfig) as (keyof AgentConfig)[];
  
  for (const key of keys) {
    if (key === "session_name" || key === "session_id" || key === "session_persistence") continue;
    
    const oldVal = oldConfig[key];
    const newVal = newConfig[key];
    
    // Deep comparison for arrays/objects
    if (JSON.stringify(oldVal) !== JSON.stringify(newVal)) {
      return true;
    }
  }
  
  return false;
}

use super::*;
use std::fs;

#[test]
fn telegram_plaintext_config_is_rejected_without_secret_in_diagnostic() {
    let directory =
        std::env::temp_dir().join(format!("surge-telegram-config-{}", crate::id::RunId::new()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("surge.toml");
    std::fs::write(
        &path,
        "[telegram]\nbot_token = \"73123:CONFIG_SECRET_LITERAL\"\nchat_id = 42\n",
    )
    .unwrap();
    let result = SurgeConfig::load(&path);
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        result.is_err(),
        "plaintext credential field silently accepted"
    );
    let error = result.unwrap_err();
    assert!(!format!("{error:?} {error}").contains("73123:CONFIG_SECRET_LITERAL"));
}

#[test]
fn test_config_discovery() {
    // Create a temporary directory structure
    let temp_dir = std::env::temp_dir().join("surge_test_discovery");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).unwrap();

    let nested_dir = temp_dir.join("subdir").join("nested");
    fs::create_dir_all(&nested_dir).unwrap();

    // Create a surge.toml in the temp_dir
    let config_path = temp_dir.join("surge.toml");
    fs::write(
        &config_path,
        r#"
default_agent = "test-agent"

[agents.test-agent]
command = "test"
"#,
    )
    .unwrap();

    // Test finding from nested directory
    let found_path = SurgeConfig::find_config_file(&nested_dir).unwrap();
    assert_eq!(found_path, config_path);

    // Test finding from the directory containing surge.toml
    let found_path = SurgeConfig::find_config_file(&temp_dir).unwrap();
    assert_eq!(found_path, config_path);

    // Test error when not found
    let non_existent_dir = std::env::temp_dir().join("surge_test_no_config");
    fs::create_dir_all(&non_existent_dir).unwrap();
    let result = SurgeConfig::find_config_file(&non_existent_dir);
    assert!(result.is_err());

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
    let _ = fs::remove_dir_all(&non_existent_dir);
}

#[test]
fn test_default_config() {
    // Test that Default provides sensible values
    let config = SurgeConfig::default();

    assert_eq!(config.default_agent, "claude-acp");
    assert!(config.agents.is_empty());
    assert_eq!(config.pipeline.max_qa_iterations, 10);
    assert_eq!(config.pipeline.max_parallel, 3);
    assert!(config.pipeline.gates.after_spec);
    assert!(config.pipeline.gates.after_plan);
    assert!(!config.pipeline.gates.after_each_subtask);
    assert!(config.pipeline.gates.after_qa);
}

#[test]
fn test_load_or_default() {
    // Create a temporary directory structure
    let temp_dir = std::env::temp_dir().join("surge_test_load_or_default");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
    fs::create_dir_all(&temp_dir).unwrap();

    // Test 1: When surge.toml exists, it should load it
    let config_path = temp_dir.join("surge.toml");
    fs::write(
        &config_path,
        r#"
default_agent = "custom-agent"

[pipeline]
max_qa_iterations = 5
max_parallel = 2
"#,
    )
    .unwrap();

    // Change to the temp directory to test load_or_default
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&temp_dir).unwrap();

    let config = SurgeConfig::discover().unwrap();
    assert_eq!(config.default_agent, "custom-agent");
    assert_eq!(config.pipeline.max_qa_iterations, 5);
    assert_eq!(config.pipeline.max_parallel, 2);

    // Test 2: When no surge.toml exists, it should return default
    let no_config_dir = std::env::temp_dir().join("surge_test_load_or_default_no_config");
    let _ = fs::remove_dir_all(&no_config_dir);
    fs::create_dir_all(&no_config_dir).unwrap();
    std::env::set_current_dir(&no_config_dir).unwrap();

    let config = SurgeConfig::discover().unwrap();
    assert_eq!(config.default_agent, "claude-acp");
    assert_eq!(config.pipeline.max_qa_iterations, 10);
    assert_eq!(config.pipeline.max_parallel, 3);

    // Restore original directory
    std::env::set_current_dir(&original_dir).unwrap();

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
    let _ = fs::remove_dir_all(&no_config_dir);
}

#[test]
fn test_config_validation() {
    // Test 1: Valid configuration passes validation
    let mut valid_config = SurgeConfig::default();
    valid_config.agents.insert(
        "claude-acp".to_string(),
        AgentConfig {
            command: "claude".to_string(),
            args: vec![],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    assert!(valid_config.validate().is_ok());

    // Test 2: default_agent not in agents map fails
    let mut invalid_config = SurgeConfig {
        default_agent: "nonexistent".to_string(),
        ..Default::default()
    };
    invalid_config.agents.insert(
        "other-agent".to_string(),
        AgentConfig {
            command: "other".to_string(),
            args: vec![],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    let result = invalid_config.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("default_agent 'nonexistent' not found"));
    assert!(err_msg.contains("Available agents: other-agent"));

    // Test 3: Empty command fails
    let mut config_empty_cmd = SurgeConfig {
        default_agent: "bad-agent".to_string(),
        ..Default::default()
    };
    config_empty_cmd.agents.insert(
        "bad-agent".to_string(),
        AgentConfig {
            command: "".to_string(),
            args: vec![],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    let result = config_empty_cmd.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Agent 'bad-agent' has empty command"));

    // Test 4: Empty TCP host fails
    let mut config_empty_host = SurgeConfig {
        default_agent: "tcp-agent".to_string(),
        ..Default::default()
    };
    config_empty_host.agents.insert(
        "tcp-agent".to_string(),
        AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "".to_string(),
                port: 8080,
            },
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    let result = config_empty_host.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Agent 'tcp-agent' TCP transport has empty host"));

    // Test 5: Invalid TCP port 0 fails
    let mut config_invalid_port = SurgeConfig {
        default_agent: "tcp-agent".to_string(),
        ..Default::default()
    };
    config_invalid_port.agents.insert(
        "tcp-agent".to_string(),
        AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "localhost".to_string(),
                port: 0,
            },
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    let result = config_invalid_port.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Agent 'tcp-agent' TCP transport has invalid port 0"));

    // Test 6: max_qa_iterations = 0 fails
    let mut config_zero_qa = SurgeConfig::default();
    config_zero_qa.pipeline.max_qa_iterations = 0;
    let result = config_zero_qa.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("pipeline.max_qa_iterations must be greater than 0"));

    // Test 7: max_parallel = 0 fails
    let mut config_zero_parallel = SurgeConfig::default();
    config_zero_parallel.pipeline.max_parallel = 0;
    let result = config_zero_parallel.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("pipeline.max_parallel must be greater than 0"));

    // Test 8: Valid TCP configuration passes
    let mut config_valid_tcp = SurgeConfig {
        default_agent: "tcp-agent".to_string(),
        ..Default::default()
    };
    config_valid_tcp.agents.insert(
        "tcp-agent".to_string(),
        AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "localhost".to_string(),
                port: 8080,
            },
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    assert!(config_valid_tcp.validate().is_ok());

    // Test 9: Empty agents map with default_agent is OK (default config scenario)
    let default_config = SurgeConfig::default();
    assert!(default_config.validate().is_ok());

    // Test 10: routing.agent_preferences referencing unknown agent fails
    let mut config_bad_routing = SurgeConfig {
        default_agent: "claude".to_string(),
        ..Default::default()
    };
    config_bad_routing.agents.insert(
        "claude".to_string(),
        AgentConfig {
            command: "claude".to_string(),
            args: vec![],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
        },
    );
    config_bad_routing
        .routing
        .agent_preferences
        .insert("complex".to_string(), "nonexistent-agent".to_string());
    let result = config_bad_routing.validate();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("routing.agent_preferences['complex']"));
    assert!(err_msg.contains("nonexistent-agent"));
}

#[test]
fn test_env_overrides() {
    // Test environment variable overrides
    // Set environment variables
    unsafe {
        std::env::set_var("SURGE_DEFAULT_AGENT", "custom-agent");
        std::env::set_var("SURGE_MAX_QA_ITERATIONS", "20");
        std::env::set_var("SURGE_MAX_PARALLEL", "5");
        std::env::set_var("SURGE_GATE_AFTER_SPEC", "false");
        std::env::set_var("SURGE_GATE_AFTER_PLAN", "false");
        std::env::set_var("SURGE_GATE_AFTER_EACH_SUBTASK", "true");
        std::env::set_var("SURGE_GATE_AFTER_QA", "false");
    }

    // Create config with defaults
    let mut config = SurgeConfig::default();

    // Verify defaults before override
    assert_eq!(config.default_agent, "claude-acp");
    assert_eq!(config.pipeline.max_qa_iterations, 10);
    assert_eq!(config.pipeline.max_parallel, 3);
    assert!(config.pipeline.gates.after_spec);
    assert!(config.pipeline.gates.after_plan);
    assert!(!config.pipeline.gates.after_each_subtask);
    assert!(config.pipeline.gates.after_qa);

    // Apply environment overrides
    config.apply_env_overrides();

    // Verify overrides were applied
    assert_eq!(config.default_agent, "custom-agent");
    assert_eq!(config.pipeline.max_qa_iterations, 20);
    assert_eq!(config.pipeline.max_parallel, 5);
    assert!(!config.pipeline.gates.after_spec);
    assert!(!config.pipeline.gates.after_plan);
    assert!(config.pipeline.gates.after_each_subtask);
    assert!(!config.pipeline.gates.after_qa);

    // Clean up environment variables
    unsafe {
        std::env::remove_var("SURGE_DEFAULT_AGENT");
        std::env::remove_var("SURGE_MAX_QA_ITERATIONS");
        std::env::remove_var("SURGE_MAX_PARALLEL");
        std::env::remove_var("SURGE_GATE_AFTER_SPEC");
        std::env::remove_var("SURGE_GATE_AFTER_PLAN");
        std::env::remove_var("SURGE_GATE_AFTER_EACH_SUBTASK");
        std::env::remove_var("SURGE_GATE_AFTER_QA");
    }

    // Test that invalid values are ignored
    unsafe {
        std::env::set_var("SURGE_MAX_QA_ITERATIONS", "invalid");
        std::env::set_var("SURGE_MAX_PARALLEL", "not-a-number");
        std::env::set_var("SURGE_GATE_AFTER_SPEC", "not-a-bool");
    }

    let mut config2 = SurgeConfig::default();
    config2.apply_env_overrides();

    // Verify invalid values were ignored (defaults remain)
    assert_eq!(config2.pipeline.max_qa_iterations, 10);
    assert_eq!(config2.pipeline.max_parallel, 3);
    assert!(config2.pipeline.gates.after_spec);

    // Clean up
    unsafe {
        std::env::remove_var("SURGE_MAX_QA_ITERATIONS");
        std::env::remove_var("SURGE_MAX_PARALLEL");
        std::env::remove_var("SURGE_GATE_AFTER_SPEC");
    }
}

#[test]
fn test_toml_serialization_minimal() {
    // Test minimal config serialization/deserialization
    let toml_str = r#"
default_agent = "test-agent"
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_agent, "test-agent");
    assert!(config.agents.is_empty());
    assert_eq!(config.pipeline.max_qa_iterations, 10);
    assert_eq!(config.pipeline.max_parallel, 3);
}

#[test]
fn test_toml_serialization_complete() {
    // Test complete config with all fields
    let toml_str = r#"
default_agent = "claude-acp"

[agents.claude-acp]
command = "claude"
args = ["--stdio"]
transport = "stdio"

[agents.copilot]
command = "gh"
args = ["copilot"]

[agents.remote]
command = "nc"
transport = { tcp = { host = "localhost", port = 9000 } }

[pipeline]
max_qa_iterations = 5
max_parallel = 2

[pipeline.gates]
after_spec = false
after_plan = true
after_each_subtask = true
after_qa = false
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();

    assert_eq!(config.default_agent, "claude-acp");
    assert_eq!(config.agents.len(), 3);

    // Check claude-code agent
    let claude = config.agents.get("claude-acp").unwrap();
    assert_eq!(claude.command, "claude");
    assert_eq!(claude.args, vec!["--stdio"]);
    assert!(matches!(claude.transport, Transport::Stdio));

    // Check copilot agent
    let copilot = config.agents.get("copilot").unwrap();
    assert_eq!(copilot.command, "gh");
    assert_eq!(copilot.args, vec!["copilot"]);

    // Check remote agent with TCP transport
    let remote = config.agents.get("remote").unwrap();
    assert_eq!(remote.command, "nc");
    if let Transport::Tcp { host, port } = &remote.transport {
        assert_eq!(host, "localhost");
        assert_eq!(*port, 9000);
    } else {
        panic!("Expected TCP transport");
    }

    // Check pipeline config
    assert_eq!(config.pipeline.max_qa_iterations, 5);
    assert_eq!(config.pipeline.max_parallel, 2);

    // Check gates
    assert!(!config.pipeline.gates.after_spec);
    assert!(config.pipeline.gates.after_plan);
    assert!(config.pipeline.gates.after_each_subtask);
    assert!(!config.pipeline.gates.after_qa);
}

#[test]
fn test_toml_deserialization_malformed() {
    // Test that malformed TOML returns error
    let bad_toml = r#"
default_agent =
command = "test"
"#;
    let result: Result<SurgeConfig, _> = toml::from_str(bad_toml);
    assert!(result.is_err());
}

#[test]
fn test_load_config_file_success() {
    // Test loading a valid config file
    let temp_dir = std::env::temp_dir().join("surge_test_load_config");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let config_path = temp_dir.join("surge.toml");
    fs::write(
        &config_path,
        r#"
default_agent = "my-agent"

[agents.my-agent]
command = "agent-binary"
args = ["--verbose"]

[pipeline]
max_qa_iterations = 15
max_parallel = 4
"#,
    )
    .unwrap();

    let config = SurgeConfig::load(&config_path).unwrap();
    assert_eq!(config.default_agent, "my-agent");
    assert_eq!(config.pipeline.max_qa_iterations, 15);
    assert_eq!(config.pipeline.max_parallel, 4);

    let agent = config.agents.get("my-agent").unwrap();
    assert_eq!(agent.command, "agent-binary");
    assert_eq!(agent.args, vec!["--verbose"]);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_load_config_file_not_found() {
    // Test loading a non-existent file
    let path = PathBuf::from("/nonexistent/path/surge.toml");
    let result = SurgeConfig::load(&path);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Failed to read"));
}

#[test]
fn test_load_config_file_invalid_toml() {
    // Test loading a file with invalid TOML
    let temp_dir = std::env::temp_dir().join("surge_test_invalid_toml");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let config_path = temp_dir.join("surge.toml");
    fs::write(&config_path, "this is not valid TOML {{{").unwrap();

    let result = SurgeConfig::load(&config_path);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Failed to parse"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_load_config_file_invalid_config() {
    // Test loading a valid TOML but invalid config (validation fails)
    let temp_dir = std::env::temp_dir().join("surge_test_invalid_config");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let config_path = temp_dir.join("surge.toml");
    fs::write(
        &config_path,
        r#"
default_agent = "missing-agent"

[agents.other-agent]
command = "test"
"#,
    )
    .unwrap();

    let result = SurgeConfig::load(&config_path);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("default_agent 'missing-agent' not found"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_transport_stdio_default() {
    // Test that Transport::Stdio is the default
    let transport = Transport::default();
    assert!(matches!(transport, Transport::Stdio));
}

#[test]
fn test_transport_tcp_serialization() {
    // Test TCP transport serialization via AgentConfig
    let toml_str = r#"
command = "test"
transport = { tcp = { host = "127.0.0.1", port = 8080 } }
"#;
    let agent: AgentConfig = toml::from_str(toml_str).unwrap();
    if let Transport::Tcp { host, port } = agent.transport {
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 8080);
    } else {
        panic!("Expected TCP transport");
    }
}

#[test]
fn test_transport_stdio_serialization() {
    // Test Stdio transport serialization via AgentConfig
    let toml_str = r#"
command = "test"
transport = "stdio"
"#;
    let agent: AgentConfig = toml::from_str(toml_str).unwrap();
    assert!(matches!(agent.transport, Transport::Stdio));
}

#[test]
fn test_gate_config_defaults() {
    // Test GateConfig default values
    let gates = GateConfig::default();
    assert!(gates.after_spec);
    assert!(gates.after_plan);
    assert!(!gates.after_each_subtask);
    assert!(gates.after_qa);
}

#[test]
fn test_pipeline_config_defaults() {
    // Test PipelineConfig default values
    let pipeline = PipelineConfig::default();
    assert_eq!(pipeline.max_qa_iterations, 10);
    assert_eq!(pipeline.max_parallel, 3);
    assert!(pipeline.gates.after_spec);
    assert!(pipeline.gates.after_plan);
    assert!(!pipeline.gates.after_each_subtask);
    assert!(pipeline.gates.after_qa);
}

#[test]
fn test_agent_config_validation_whitespace_command() {
    // Test that whitespace-only command fails validation
    let agent = AgentConfig {
        command: "   ".to_string(),
        args: vec![],
        transport: Transport::Stdio,
        mcp_servers: vec![],
        capabilities: vec![],
        env: std::collections::BTreeMap::new(),
        settings_files: vec![],
    };
    let result = agent.validate("test-agent");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("test-agent"));
    assert!(err_msg.contains("empty command"));
}

#[test]
fn test_agent_config_validation_whitespace_tcp_host() {
    // Test that whitespace-only TCP host fails validation
    let agent = AgentConfig {
        command: "test".to_string(),
        args: vec![],
        transport: Transport::Tcp {
            host: "   ".to_string(),
            port: 8080,
        },
        mcp_servers: vec![],
        capabilities: vec![],
        env: std::collections::BTreeMap::new(),
        settings_files: vec![],
    };
    let result = agent.validate("test-agent");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("test-agent"));
    assert!(err_msg.contains("empty host"));
}

#[test]
fn test_pipeline_config_validation_success() {
    // Test that valid pipeline config passes validation
    let pipeline = PipelineConfig {
        max_qa_iterations: 5,
        max_parallel: 10,
        gates: GateConfig::default(),
        max_cost_usd: None,
        max_tokens: None,
    };
    assert!(pipeline.validate().is_ok());
}

#[test]
fn test_agent_config_with_args() {
    // Test agent config with multiple arguments
    let toml_str = r#"
command = "gh"
args = ["copilot", "suggest", "--verbose"]
"#;
    let agent: AgentConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(agent.command, "gh");
    assert_eq!(agent.args, vec!["copilot", "suggest", "--verbose"]);
    assert!(matches!(agent.transport, Transport::Stdio));
}

#[test]
fn test_config_clone() {
    // Test that SurgeConfig can be cloned
    let config = SurgeConfig {
        default_agent: "custom".to_string(),
        pipeline: crate::config::PipelineConfig {
            max_qa_iterations: 42,
            ..Default::default()
        },
        ..Default::default()
    };

    let cloned = config.clone();
    assert_eq!(cloned.default_agent, "custom");
    assert_eq!(cloned.pipeline.max_qa_iterations, 42);
}

#[test]
fn test_transport_clone() {
    // Test that Transport can be cloned
    let tcp = Transport::Tcp {
        host: "localhost".to_string(),
        port: 9000,
    };
    let cloned = tcp.clone();
    if let Transport::Tcp { host, port } = cloned {
        assert_eq!(host, "localhost");
        assert_eq!(port, 9000);
    } else {
        panic!("Expected TCP transport");
    }
}

#[test]
fn test_toml_partial_pipeline() {
    // Test TOML with partial pipeline config (uses defaults for missing fields)
    let toml_str = r#"
default_agent = "test"

[pipeline]
max_qa_iterations = 7

[pipeline.gates]
after_spec = false
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.pipeline.max_qa_iterations, 7);
    assert_eq!(config.pipeline.max_parallel, 3); // default
    assert!(!config.pipeline.gates.after_spec);
    assert!(config.pipeline.gates.after_plan); // default
}

#[test]
fn test_empty_agents_map_validation() {
    // Test that empty agents map with any default_agent is valid
    let config = SurgeConfig {
        default_agent: "nonexistent".to_string(),
        ..Default::default()
    };
    // Should be valid because agents map is empty
    assert!(config.validate().is_ok());
}

#[test]
fn test_tcp_transport_valid_port_range() {
    // Test valid TCP port ranges
    let agent_min = AgentConfig {
        command: "test".to_string(),
        args: vec![],
        transport: Transport::Tcp {
            host: "localhost".to_string(),
            port: 1,
        },
        mcp_servers: vec![],
        capabilities: vec![],
        env: std::collections::BTreeMap::new(),
        settings_files: vec![],
    };
    assert!(agent_min.validate("test").is_ok());

    let agent_max = AgentConfig {
        command: "test".to_string(),
        args: vec![],
        transport: Transport::Tcp {
            host: "localhost".to_string(),
            port: 65535,
        },
        mcp_servers: vec![],
        capabilities: vec![],
        env: std::collections::BTreeMap::new(),
        settings_files: vec![],
    };
    assert!(agent_max.validate("test").is_ok());
}

#[test]
fn test_routing_config_defaults() {
    let config = RoutingConfig::default();
    assert_eq!(config.strategy, RoutingStrategy::Default);
    assert!(config.agent_preferences.is_empty());
}

#[test]
fn test_cleanup_policy_defaults() {
    let policy = CleanupPolicy::default();
    assert!(policy.remove_worktrees_on_complete);
    assert_eq!(policy.keep_branches_days, 7);
}

#[test]
fn test_extended_config_toml_roundtrip() {
    let toml_str = r#"
default_agent = "claude"

[agents.claude]
command = "claude"

[pipeline]
max_qa_iterations = 10
max_parallel = 3

[routing]
strategy = "default"

[cleanup]
remove_worktrees_on_complete = false
keep_branches_days = 14

[ide]
editor = "rustrover"
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.routing.strategy, RoutingStrategy::Default);
    assert!(!config.cleanup.remove_worktrees_on_complete);
    assert_eq!(config.cleanup.keep_branches_days, 14);
    assert_eq!(config.ide.editor, Some("rustrover".to_string()));
}

#[test]
fn test_extended_config_missing_sections_use_defaults() {
    let toml_str = r#"default_agent = "test""#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.routing.strategy, RoutingStrategy::Default);
    assert!(config.cleanup.remove_worktrees_on_complete);
    assert_eq!(config.cleanup.keep_branches_days, 7);
    assert!(config.ide.editor.is_none());
}

#[test]
fn test_websocket_transport_roundtrip() {
    let _toml_str = r#"
command = "agent"
[transport]
url = "ws://localhost:8080"
"#;
    // Deserialize using the "ws" tag
    let _toml_str2 = r#"transport = {ws = {url = "ws://localhost:8080"}}"#;
    // Use inline table format that matches serde rename
    let agent: AgentConfig =
        toml::from_str("command = \"agent\"\n[transport.ws]\nurl = \"ws://localhost:8080\"\n")
            .unwrap();
    assert!(matches!(agent.transport, Transport::WebSocket { .. }));

    let serialized = toml::to_string(&agent).unwrap();
    let roundtripped: AgentConfig = toml::from_str(&serialized).unwrap();
    assert!(
        matches!(roundtripped.transport, Transport::WebSocket { url } if url == "ws://localhost:8080")
    );
}

#[test]
fn test_websocket_transport_validation_error() {
    let agent = AgentConfig {
        command: "agent".to_string(),
        args: vec![],
        transport: Transport::WebSocket {
            url: "ws://localhost:8080".to_string(),
        },
        mcp_servers: vec![],
        capabilities: vec![],
        env: std::collections::BTreeMap::new(),
        settings_files: vec![],
    };
    let err = agent.validate("test-agent").unwrap_err();
    assert!(
        err.to_string()
            .contains("WebSocket transport not yet supported")
    );
}

#[test]
fn test_pipeline_token_budget_defaults_to_none() {
    let pipeline = PipelineConfig::default();
    assert!(pipeline.max_cost_usd.is_none());
    assert!(pipeline.max_tokens.is_none());
}

#[test]
fn analytics_budget_guard_default_is_unlimited() {
    let guard = AnalyticsConfig::default().budget_guard();
    assert!(guard.is_unlimited());
    assert_eq!(guard.policy, crate::budget::BudgetPolicy::Abort);
}

#[test]
fn analytics_budget_guard_resolves_limits() {
    let analytics = AnalyticsConfig {
        budget_usd: Some(25.0),
        budget_tokens: Some(500_000),
        budget_warn_threshold: 75,
        ..AnalyticsConfig::default()
    };
    let guard = analytics.budget_guard();
    assert!(!guard.is_unlimited());
    assert_eq!(guard.limits.usd, Some(25.0));
    assert_eq!(guard.limits.tokens, Some(500_000));
    assert_eq!(guard.limits.warn_threshold_pct, 75);
}

#[test]
fn test_pipeline_token_budget_roundtrip() {
    let toml_str = r#"
default_agent = "test"

[pipeline]
max_cost_usd = 1.50
max_tokens = 100000
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.pipeline.max_cost_usd, Some(1.50));
    assert_eq!(config.pipeline.max_tokens, Some(100_000));

    let serialized = toml::to_string(&config).unwrap();
    let roundtripped: SurgeConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(roundtripped.pipeline.max_cost_usd, Some(1.50));
    assert_eq!(roundtripped.pipeline.max_tokens, Some(100_000));
}

#[test]
fn test_pipeline_budget_omitted_when_none() {
    let config = SurgeConfig::default();
    let s = toml::to_string(&config).unwrap();
    assert!(!s.contains("max_cost_usd"));
    assert!(!s.contains("max_tokens"));
}

#[test]
fn test_log_config_defaults() {
    let log = LogConfig::default();
    assert_eq!(log.level, "info");
    assert!(log.file.is_none());
    assert_eq!(log.max_size_mb, 50);
}

#[test]
fn test_log_config_roundtrip() {
    let toml_str = r#"
default_agent = "test"

[log]
level = "debug"
file = "/var/log/surge.log"
max_size_mb = 100
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.log.level, "debug");
    assert_eq!(
        config.log.file.as_ref().unwrap().to_str().unwrap(),
        "/var/log/surge.log"
    );
    assert_eq!(config.log.max_size_mb, 100);
}

#[test]
fn test_ide_config_extension_defaults() {
    let ide = IdeConfig::default();
    assert!(ide.editor.is_none());
    assert!(ide.open_file_cmd.is_none());
    assert!(!ide.auto_open_worktree);
}

#[test]
fn test_ide_config_extension_roundtrip() {
    let toml_str = r#"
default_agent = "test"

[ide]
editor = "vscode"
open_file_cmd = "code {path}:{line}"
auto_open_worktree = true
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.ide.editor.as_deref(), Some("vscode"));
    assert_eq!(
        config.ide.open_file_cmd.as_deref(),
        Some("code {path}:{line}")
    );
    assert!(config.ide.auto_open_worktree);
}

#[test]
fn test_log_level_validation_valid() {
    for level in ["error", "warn", "info", "debug", "trace"] {
        let mut config = SurgeConfig::default();
        config.log.level = level.to_string();
        assert!(config.validate().is_ok(), "level '{level}' should be valid");
    }
}

#[test]
fn test_log_level_validation_invalid() {
    let mut config = SurgeConfig::default();
    config.log.level = "verbose".to_string();
    let err = config.validate().unwrap_err();
    assert!(err.to_string().contains("verbose"));
}

#[test]
fn test_example_toml_deserializes() {
    let content = include_str!("../../../../surge.example.toml");
    let config: SurgeConfig = toml::from_str(content).unwrap();
    assert_eq!(config.default_agent, "claude");
    assert!(config.agents.contains_key("claude"));
    config.validate().unwrap();
}

#[test]
fn test_analytics_config_defaults() {
    let config = AnalyticsConfig::default();
    assert_eq!(config.default_pricing.currency, "USD");
    assert!(
        config
            .default_pricing
            .input_cost_per_million_tokens
            .is_none()
    );
    assert!(
        config
            .default_pricing
            .output_cost_per_million_tokens
            .is_none()
    );
    assert!(config.budget_usd.is_none());
    assert!(config.budget_tokens.is_none());
    assert_eq!(config.budget_warn_threshold, 80);
}

#[test]
fn test_analytics_config_roundtrip() {
    let toml_str = r#"
[analytics]
budget_usd = 100.0
budget_tokens = 1000000
budget_warn_threshold = 90

[analytics.default_pricing]
input_cost_per_million_tokens = 3.0
output_cost_per_million_tokens = 15.0
currency = "USD"
"#;
    let config: SurgeConfig =
        toml::from_str(&format!("default_agent = \"test\"\n{}", toml_str)).unwrap();
    assert_eq!(config.analytics.budget_usd, Some(100.0));
    assert_eq!(config.analytics.budget_tokens, Some(1000000));
    assert_eq!(config.analytics.budget_warn_threshold, 90);
    assert_eq!(
        config
            .analytics
            .default_pricing
            .input_cost_per_million_tokens,
        Some(3.0)
    );
    assert_eq!(
        config
            .analytics
            .default_pricing
            .output_cost_per_million_tokens,
        Some(15.0)
    );
    assert_eq!(config.analytics.default_pricing.currency, "USD");
}

#[test]
fn test_analytics_config_optional_fields() {
    // Test that analytics section with no fields uses defaults
    let toml_str = r#"
default_agent = "test"

[analytics]
"#;
    let config: SurgeConfig = toml::from_str(toml_str).unwrap();
    assert!(config.analytics.budget_usd.is_none());
    assert!(config.analytics.budget_tokens.is_none());
    assert_eq!(config.analytics.budget_warn_threshold, 80);
}

#[test]
fn test_gate_decision_approved() {
    let decision = GateDecision::Approved {
        feedback: Some("Looks good".to_string()),
    };
    assert!(decision.is_approved());
    assert!(!decision.is_rejected());
    assert!(!decision.is_timeout());
    assert!(!decision.is_aborted());
    assert!(!decision.is_terminal());
    assert!(decision.rejection_feedback().is_none());
}

#[test]
fn test_gate_decision_rejected() {
    let decision = GateDecision::Rejected {
        reason: "Wrong approach".to_string(),
        feedback: "Use async instead".to_string(),
    };
    assert!(!decision.is_approved());
    assert!(decision.is_rejected());
    assert!(!decision.is_timeout());
    assert!(!decision.is_aborted());
    assert!(!decision.is_terminal());
    assert_eq!(decision.rejection_feedback(), Some("Use async instead"));
    assert_eq!(decision.reason(), Some("Wrong approach"));
}

#[test]
fn test_gate_decision_timeout() {
    let decision = GateDecision::Timeout {
        context: Some("No response after 2 hours".to_string()),
    };
    assert!(!decision.is_approved());
    assert!(!decision.is_rejected());
    assert!(decision.is_timeout());
    assert!(!decision.is_aborted());
    assert!(decision.is_terminal());
    assert!(decision.rejection_feedback().is_none());
}

#[test]
fn test_gate_decision_aborted() {
    let decision = GateDecision::Aborted {
        reason: "User cancelled".to_string(),
    };
    assert!(!decision.is_approved());
    assert!(!decision.is_rejected());
    assert!(!decision.is_timeout());
    assert!(decision.is_aborted());
    assert!(decision.is_terminal());
    assert!(decision.rejection_feedback().is_none());
    assert_eq!(decision.reason(), Some("User cancelled"));
}

#[test]
fn test_gate_decision_mutually_exclusive() {
    // Each decision type should match exactly one category
    let decisions = vec![
        GateDecision::Approved { feedback: None },
        GateDecision::Rejected {
            reason: "test".to_string(),
            feedback: "fix".to_string(),
        },
        GateDecision::Timeout { context: None },
        GateDecision::Aborted {
            reason: "test".to_string(),
        },
    ];

    for decision in decisions {
        let flags = [
            decision.is_approved(),
            decision.is_rejected(),
            decision.is_timeout(),
            decision.is_aborted(),
        ];
        let true_count = flags.iter().filter(|&&b| b).count();
        assert_eq!(
            true_count, 1,
            "Decision {:?} matched {} categories",
            decision, true_count
        );
    }
}

#[test]
fn test_gate_decision_terminal_states() {
    // Terminal states: timeout and abort
    assert!(GateDecision::Timeout { context: None }.is_terminal());
    assert!(
        GateDecision::Aborted {
            reason: "test".into()
        }
        .is_terminal()
    );

    // Non-terminal states: approved and rejected
    assert!(!GateDecision::Approved { feedback: None }.is_terminal());
    assert!(
        !GateDecision::Rejected {
            reason: "test".into(),
            feedback: "fix".into()
        }
        .is_terminal()
    );
}

#[test]
fn task_sources_round_trip_toml() {
    let toml_str = r#"
default_agent = "claude-acp"

[[task_sources]]
type = "linear"
id = "linear-acme"
workspace_id = "wsp_acme_123"
api_token_env = "LINEAR_API_TOKEN"
poll_interval_seconds = 60
label_filters = ["surge:enabled", "surge:auto"]

[[task_sources]]
type = "github_issues"
id = "github-myapp"
repo = "user/myapp"
api_token_env = "GITHUB_TOKEN"
poll_interval_seconds = 120
label_filters = ["surge:enabled"]
"#;
    let cfg: SurgeConfig = toml::from_str(toml_str).expect("parse SurgeConfig");
    assert_eq!(cfg.task_sources.len(), 2);
    match &cfg.task_sources[0] {
        TaskSourceConfig::Linear(l) => {
            assert_eq!(l.id, "linear-acme");
            assert_eq!(l.workspace_id, "wsp_acme_123");
            assert_eq!(l.api_token_env, "LINEAR_API_TOKEN");
            assert_eq!(l.poll_interval, std::time::Duration::from_secs(60));
            assert_eq!(l.label_filters.len(), 2);
            assert_eq!(l.label_filters[0], "surge:enabled");
            assert_eq!(l.label_filters[1], "surge:auto");
        },
        other => panic!("expected Linear, got {other:?}"),
    }
    match &cfg.task_sources[1] {
        TaskSourceConfig::GithubIssues(g) => {
            assert_eq!(g.id, "github-myapp");
            assert_eq!(g.repo, "user/myapp");
            assert_eq!(g.api_token_env, "GITHUB_TOKEN");
            assert_eq!(g.poll_interval, std::time::Duration::from_secs(120));
            assert_eq!(g.label_filters.len(), 1);
            assert_eq!(g.label_filters[0], "surge:enabled");
            // Omitted in the TOML above → defaults to squash.
            assert_eq!(g.merge_method, MergeMethod::Squash);
        },
        other => panic!("expected GithubIssues, got {other:?}"),
    }
}

#[test]
fn github_merge_method_default_is_squash_and_overridable() {
    // Default when the key is absent.
    assert_eq!(MergeMethod::default(), MergeMethod::Squash);

    let toml_str = r#"
default_agent = "claude-acp"

[[task_sources]]
type = "github_issues"
id = "gh"
repo = "user/repo"
api_token_env = "GITHUB_TOKEN"
merge_method = "rebase"
"#;
    let cfg: SurgeConfig = toml::from_str(toml_str).expect("parse");
    match &cfg.task_sources[0] {
        TaskSourceConfig::GithubIssues(g) => {
            assert_eq!(g.merge_method, MergeMethod::Rebase);
        },
        other => panic!("expected GithubIssues, got {other:?}"),
    }
}

#[test]
fn poll_interval_default_is_60s() {
    let toml_str = r#"
default_agent = "claude-acp"

[[task_sources]]
type = "linear"
id = "x"
workspace_id = "y"
api_token_env = "LINEAR_API_TOKEN"
"#;
    let cfg: SurgeConfig = toml::from_str(toml_str).expect("parse");
    match &cfg.task_sources[0] {
        TaskSourceConfig::Linear(l) => {
            assert_eq!(l.poll_interval, std::time::Duration::from_secs(60));
        },
        _ => unreachable!(),
    }
}

#[test]
fn empty_task_sources_is_default() {
    let toml_str = r#"default_agent = "claude-acp""#;
    let cfg: SurgeConfig = toml::from_str(toml_str).expect("parse empty");
    assert!(cfg.task_sources.is_empty());
}

#[test]
fn telegram_and_inbox_config_round_trip_toml() {
    let toml_str = r#"
[telegram]
chat_id_env = "SURGE_TELEGRAM_CHAT_ID"
bot_token_env = "SURGE_TELEGRAM_BOT_TOKEN"

[inbox]
snooze_poll_interval_seconds = 600
delivery_channels = ["telegram", "desktop"]
"#;
    let cfg: SurgeConfig = toml::from_str(toml_str).expect("must parse");
    let telegram = cfg.telegram.expect("telegram section");
    assert_eq!(
        telegram.chat_id_env.as_deref(),
        Some("SURGE_TELEGRAM_CHAT_ID")
    );
    assert_eq!(
        telegram.bot_token_env.as_deref(),
        Some("SURGE_TELEGRAM_BOT_TOKEN")
    );
    assert_eq!(telegram.chat_id, None);
    let inbox = cfg.inbox;
    assert_eq!(inbox.snooze_poll_interval.as_secs(), 600);
    assert_eq!(
        inbox.delivery_channels,
        vec!["telegram".to_string(), "desktop".to_string()]
    );
}

#[test]
fn telegram_and_inbox_config_default_when_absent() {
    let cfg: SurgeConfig = toml::from_str("").unwrap();
    assert!(cfg.telegram.is_none());
    assert_eq!(cfg.inbox.snooze_poll_interval.as_secs(), 300);
    assert!(cfg.inbox.delivery_channels.is_empty());
}

#[test]
fn mcp_servers_default_to_empty_vec() {
    let cfg: SurgeConfig = toml::from_str("").unwrap();
    assert!(cfg.mcp_servers.is_empty());
}

#[test]
fn mcp_servers_parse_from_toml_array_of_tables() {
    let toml_src = r#"
[[mcp_servers]]
name = "playwright"
transport = { kind = "stdio", command = "/usr/local/bin/mcp-playwright", args = ["--headless"] }
allowed_tools = ["browser_navigate", "browser_screenshot"]
call_timeout = "120s"
restart_on_crash = false

[[mcp_servers]]
name = "github"
transport = { kind = "stdio", command = "npx", args = ["@github/mcp-server"] }
"#;
    let cfg: SurgeConfig = toml::from_str(toml_src).unwrap();
    assert_eq!(cfg.mcp_servers.len(), 2);
    let first = &cfg.mcp_servers[0];
    assert_eq!(first.name, "playwright");
    assert_eq!(
        first.allowed_tools.as_deref(),
        Some(
            &[
                "browser_navigate".to_string(),
                "browser_screenshot".to_string(),
            ][..]
        )
    );
    assert_eq!(first.call_timeout, std::time::Duration::from_secs(120));
    assert!(!first.restart_on_crash);
    // Second entry exercises defaults (no allowed_tools / call_timeout /
    // restart_on_crash specified in TOML).
    let second = &cfg.mcp_servers[1];
    assert_eq!(second.name, "github");
    assert!(second.allowed_tools.is_none());
    assert_eq!(second.call_timeout, std::time::Duration::from_secs(60));
    assert!(second.restart_on_crash);
}

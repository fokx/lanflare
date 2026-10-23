use clap::Parser;
use lanflare::{ChatCommands, Cli, Commands, MeshCommands};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;

#[tokio::test]
async fn test_cli_mesh_and_chat_arg_parsing_and_execution() {
    // ------------------------------------------------------------------------
    // 1. Argument Parsing Verification for all 8 subcommands
    // ------------------------------------------------------------------------

    // A: lanflare mesh status
    let cli = Cli::try_parse_from(["lanflare", "mesh", "status"]).expect("parse mesh status");
    match cli.command {
        Some(Commands::Mesh(args)) => match args.command {
            MeshCommands::Status(s) => assert!(!s.relay),
            _ => panic!("expected MeshCommands::Status"),
        },
        _ => panic!("expected Commands::Mesh"),
    }

    let cli_relay = Cli::try_parse_from(["lanflare", "mesh", "status", "--relay"])
        .expect("parse mesh status --relay");
    match cli_relay.command {
        Some(Commands::Mesh(args)) => match args.command {
            MeshCommands::Status(s) => assert!(s.relay),
            _ => panic!("expected MeshCommands::Status"),
        },
        _ => panic!("expected Commands::Mesh"),
    }

    // B: lanflare mesh sim
    let cli_sim = Cli::try_parse_from([
        "lanflare", "mesh", "sim", "-n", "3", "-d", "2", "--subnets", "SubA,SubB", "--latency-ms", "15",
    ])
    .expect("parse mesh sim");
    match cli_sim.command {
        Some(Commands::Mesh(args)) => match args.command {
            MeshCommands::Sim(sim) => {
                assert_eq!(sim.nodes, 3);
                assert_eq!(sim.duration, 2);
                assert_eq!(sim.subnets, "SubA,SubB");
                assert_eq!(sim.latency_ms, 15);
            }
            _ => panic!("expected MeshCommands::Sim"),
        },
        _ => panic!("expected Commands::Mesh"),
    }

    // C: lanflare mesh trace <target>
    let cli_trace = Cli::try_parse_from(["lanflare", "mesh", "trace", "NodeAlpha", "--ttl", "4"])
        .expect("parse mesh trace");
    match cli_trace.command {
        Some(Commands::Mesh(args)) => match args.command {
            MeshCommands::Trace(trace) => {
                assert_eq!(trace.target, "NodeAlpha");
                assert_eq!(trace.ttl, 4);
            }
            _ => panic!("expected MeshCommands::Trace"),
        },
        _ => panic!("expected Commands::Mesh"),
    }

    // D: lanflare mesh benchmark <target>
    let cli_bench = Cli::try_parse_from([
        "lanflare", "mesh", "benchmark", "NodeBeta", "-n", "8", "-s", "65536",
    ])
    .expect("parse mesh benchmark");
    match cli_bench.command {
        Some(Commands::Mesh(args)) => match args.command {
            MeshCommands::Benchmark(bench) => {
                assert_eq!(bench.target, "NodeBeta");
                assert_eq!(bench.chunks, 8);
                assert_eq!(bench.chunk_size, 65536);
            }
            _ => panic!("expected MeshCommands::Benchmark"),
        },
        _ => panic!("expected Commands::Mesh"),
    }

    // E: lanflare chat send <target> <message>
    let cli_chat_send = Cli::try_parse_from([
        "lanflare", "chat", "send", "Bob", "Hello secret friend!", "--intro",
    ])
    .expect("parse chat send");
    match cli_chat_send.command {
        Some(Commands::Chat(args)) => match args.command {
            ChatCommands::Send(send) => {
                assert_eq!(send.target, "Bob");
                assert_eq!(send.message, "Hello secret friend!");
                assert!(send.intro);
            }
            _ => panic!("expected ChatCommands::Send"),
        },
        _ => panic!("expected Commands::Chat"),
    }

    // F: lanflare chat trust <target>
    let cli_trust_app = Cli::try_parse_from(["lanflare", "chat", "trust", "Alice", "--approve"])
        .expect("parse chat trust --approve");
    match cli_trust_app.command {
        Some(Commands::Chat(args)) => match args.command {
            ChatCommands::Trust(trust) => {
                assert_eq!(trust.target, "Alice");
                assert!(trust.approve);
                assert!(!trust.block);
            }
            _ => panic!("expected ChatCommands::Trust"),
        },
        _ => panic!("expected Commands::Chat"),
    }

    let cli_trust_blk = Cli::try_parse_from(["lanflare", "chat", "trust", "Eve", "--block"])
        .expect("parse chat trust --block");
    match cli_trust_blk.command {
        Some(Commands::Chat(args)) => match args.command {
            ChatCommands::Trust(trust) => {
                assert_eq!(trust.target, "Eve");
                assert!(!trust.approve);
                assert!(trust.block);
            }
            _ => panic!("expected ChatCommands::Trust"),
        },
        _ => panic!("expected Commands::Chat"),
    }

    // G: lanflare chat groups
    let cli_groups = Cli::try_parse_from(["lanflare", "chat", "groups", "--id", "grp-789"])
        .expect("parse chat groups");
    match cli_groups.command {
        Some(Commands::Chat(args)) => match args.command {
            ChatCommands::Groups(grp) => {
                assert_eq!(grp.id.as_deref(), Some("grp-789"));
            }
            _ => panic!("expected ChatCommands::Groups"),
        },
        _ => panic!("expected Commands::Chat"),
    }

    // H: lanflare search <query>
    let cli_search = Cli::try_parse_from([
        "lanflare", "search", "confidential_specs", "--pin", "4321", "--timeout", "5",
    ])
    .expect("parse search");
    match cli_search.command {
        Some(Commands::Search(search)) => {
            assert_eq!(search.query, "confidential_specs");
            assert_eq!(search.pin.as_deref(), Some("4321"));
            assert_eq!(search.timeout, 5);
        }
        _ => panic!("expected Commands::Search"),
    }

    // ------------------------------------------------------------------------
    // 2. Command Execution Handlers Verification
    // ------------------------------------------------------------------------
    let config = AppConfig::default();
    let identity = generate_self_signed().expect("generate identity");

    // Test mesh_status handler
    let status_opts = lanflare::commands::MeshStatusOptions { relay: true };
    let status_res = lanflare::commands::mesh_status(&config, &identity, &status_opts).await;
    assert!(status_res.is_ok(), "mesh_status execution failed: {:?}", status_res.err());

    // Test mesh_sim handler with minimal 2 nodes and short 1-sec duration
    let sim_opts = lanflare::commands::MeshSimOptions {
        nodes: 2,
        duration: 1,
        subnets: vec!["SubnetA".to_string()],
        latency_ms: 5,
    };
    let sim_res = lanflare::commands::mesh_sim(&config, &identity, &sim_opts).await;
    assert!(sim_res.is_ok(), "mesh_sim execution failed: {:?}", sim_res.err());

    // Test chat_groups handler (empty groups list)
    let groups_opts = lanflare::commands::ChatGroupsOptions { id: None };
    let groups_res = lanflare::commands::chat_groups(&config, &identity, &groups_opts).await;
    assert!(groups_res.is_ok(), "chat_groups execution failed: {:?}", groups_res.err());

    // Test chat_trust handler (inspect stranger contact)
    let trust_opts = lanflare::commands::ChatTrustOptions {
        target: "2233445566778899001122334455667788990011223344556677889900112233".to_string(),
        approve: false,
        block: false,
    };
    let trust_res = lanflare::commands::chat_trust(&config, &identity, &trust_opts).await;
    assert!(trust_res.is_ok(), "chat_trust execution failed: {:?}", trust_res.err());

    // Test search handler across local directory
    let search_opts = lanflare::commands::SearchOptions {
        query: "nonexistent_file_query_xyz".to_string(),
        pin: None,
        timeout: 1,
    };
    let search_res = lanflare::commands::search(&config, &identity, &search_opts).await;
    assert!(search_res.is_ok(), "search execution failed: {:?}", search_res.err());
}

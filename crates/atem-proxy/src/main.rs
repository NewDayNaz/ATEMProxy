use anyhow::{Context, Result};
use atem_proxy::{run_proxy, Config};
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "atem-proxy",
    about = "Transparent multi-client Blackmagic ATEM UDP proxy",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to TOML config file
    #[arg(long, global = true, env = "ATEM_PROXY_CONFIG")]
    config: Option<PathBuf>,

    /// Upstream ATEM host or host:port
    #[arg(long, global = true, env = "ATEM_PROXY_ATEM")]
    atem: Option<String>,

    /// Client-facing bind address
    #[arg(long, global = true, env = "ATEM_PROXY_BIND")]
    bind: Option<SocketAddr>,

    /// Enable mDNS announcement
    #[arg(long, global = true, env = "ATEM_PROXY_MDNS", num_args = 0..=1, default_missing_value = "true")]
    mdns: Option<bool>,

    /// SoftAtem just-works profile (locks + media + mDNS)
    #[arg(long, global = true, env = "ATEM_PROXY_SOFTATEM", num_args = 0..=1, default_missing_value = "true")]
    softatem: Option<bool>,

    /// Log filter (e.g. info, atem_proxy=debug)
    #[arg(long, global = true, env = "ATEM_PROXY_LOG")]
    log: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the proxy in the foreground (default)
    Run,
    /// Windows service management
    #[cfg(windows)]
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[cfg(windows)]
#[derive(Debug, Subcommand)]
enum ServiceAction {
    Install {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Uninstall,
    Start,
    Stop,
    /// Entry point used by the Service Control Manager
    Run,
}

fn init_logging(filter: &str, log_file: Option<&std::path::Path>) {
    let env = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));
    if let Some(path) = log_file {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env)
                .with_target(false)
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
            return;
        }
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env)
        .with_target(false)
        .try_init();
}

fn load_cfg(cli: &Cli) -> Result<Config> {
    let mut cfg = Config::load(
        cli.config.as_deref(),
        cli.atem.clone(),
        cli.bind,
        cli.mdns,
        cli.log.clone(),
    )?;
    if let Some(v) = cli.softatem {
        cfg.compat.softatem = v;
        cfg.normalize();
    }
    Ok(cfg)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Run) => {
            let cfg = load_cfg(&cli)?;
            init_logging(&cfg.log, cfg.log_file.as_deref());
            let cancel = CancellationToken::new();
            let cancel_c = cancel.clone();
            ctrlc_cancel(cancel_c);
            run_proxy(cfg, cancel).await
        }
        #[cfg(windows)]
        Some(Commands::Service { action }) => match action {
            ServiceAction::Install { config } => {
                windows_svc::install(config.or_else(|| cli.config.clone()))
            }
            ServiceAction::Uninstall => windows_svc::uninstall(),
            ServiceAction::Start => windows_svc::start(),
            ServiceAction::Stop => windows_svc::stop(),
            ServiceAction::Run => {
                if let Some(path) = &cli.config {
                    std::env::set_var("ATEM_PROXY_CONFIG", path);
                }
                windows_svc::run_service()
            }
        },
    }
}

fn ctrlc_cancel(cancel: CancellationToken) {
    let installed = Arc::new(AtomicBool::new(false));
    let flag = installed.clone();
    let _ = ctrlc_impl(move || {
        if !flag.swap(true, Ordering::SeqCst) {
            cancel.cancel();
        }
    });
}

fn ctrlc_impl<F: Fn() + Send + 'static>(f: F) -> Result<()> {
    // Use tokio signal instead of ctrlc crate to avoid extra dep.
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).ok();
            let mut sigint = signal(SignalKind::interrupt()).ok();
            tokio::select! {
                _ = async {
                    if let Some(s) = sigterm.as_mut() { s.recv().await; }
                    else { std::future::pending::<()>().await; }
                } => {}
                _ = async {
                    if let Some(s) = sigint.as_mut() { s.recv().await; }
                    else { tokio::signal::ctrl_c().await.ok(); }
                } => {}
            }
            f();
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            f();
        }
    });
    Ok(())
}

#[cfg(windows)]
mod windows_svc {
    use super::*;
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    const SERVICE_NAME: &str = "AtemProxy";
    const SERVICE_DISPLAY: &str = "ATEM Proxy";

    pub fn install(config: Option<PathBuf>) -> Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )?;
        let exe = std::env::current_exe().context("current_exe")?;
        let mut args = vec![OsString::from("service"), OsString::from("run")];
        let cfg_path = config.unwrap_or_else(Config::default_windows_config_path);
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if !cfg_path.exists() {
            let example = toml::to_string_pretty(&Config::default())?;
            std::fs::write(&cfg_path, example)?;
            println!("Wrote default config to {}", cfg_path.display());
        }
        args.push(OsString::from("--config"));
        args.push(cfg_path.into_os_string());

        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe,
            launch_arguments: args,
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };
        let service =
            manager.create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)?;
        service.set_description("Transparent multi-client Blackmagic ATEM UDP proxy")?;
        println!("Installed Windows service '{SERVICE_NAME}'");
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service =
            manager.open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP)?;
        let _ = service.stop();
        service.delete()?;
        println!("Uninstalled Windows service '{SERVICE_NAME}'");
        Ok(())
    }

    pub fn start() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::START)?;
        service.start::<&str>(&[])?;
        println!("Started '{SERVICE_NAME}'");
        Ok(())
    }

    pub fn stop() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::STOP)?;
        service.stop()?;
        println!("Stopped '{SERVICE_NAME}'");
        Ok(())
    }

    define_windows_service!(ffi_service_main, service_main);

    pub fn run_service() -> Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("service_dispatcher::start")?;
        Ok(())
    }

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service_body() {
            eprintln!("atem-proxy service error: {e:#}");
        }
    }

    fn run_service_body() -> Result<()> {
        let cancel = CancellationToken::new();
        let cancel_c = cancel.clone();

        let event_handler = move |control| match control {
            windows_service::service::ServiceControl::Stop
            | windows_service::service::ServiceControl::Shutdown => {
                cancel_c.cancel();
                ServiceControlHandlerResult::NoError
            }
            windows_service::service::ServiceControl::Interrogate => {
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;
        status_handle.set_service_status(windows_service::service::ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: windows_service::service::ServiceControlAccept::STOP
                | windows_service::service::ServiceControlAccept::SHUTDOWN,
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        let cfg_path = std::env::var_os("ATEM_PROXY_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(Config::default_windows_config_path);
        let mut cfg = Config::load(Some(&cfg_path), None, None, None, None)?;
        if cfg.log_file.is_none() {
            let base = std::env::var_os("ProgramData")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
            cfg.log_file = Some(base.join("AtemProxy").join("atem-proxy.log"));
        }
        init_logging(&cfg.log, cfg.log_file.as_deref());

        let rt = tokio::runtime::Runtime::new()?;
        let result = rt.block_on(run_proxy(cfg, cancel));

        status_handle.set_service_status(windows_service::service::ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: windows_service::service::ServiceControlAccept::empty(),
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;
        result
    }
}

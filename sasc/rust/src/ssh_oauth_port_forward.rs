// ssh-oauth-port-forward.asi [CGE v35.43-Ω Φ^∞ SSH_OAUTH → TOKEN_BASED_PORT_FORWARDING]
// BLOCK #122.4→130 | 289 NODES | χ=2 LOCAL_REMOTE_DYNAMIC | QUARTO CAMINHO RAMO A

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use crate::cge_log;

// ============================================================================
// CONSTANTES E CONFIGURAÇÕES
// ============================================================================

pub const MAX_TUNNELS_PER_TYPE: usize = 12;
pub const TOKEN_BUFFER_SIZE: usize = 4096;
pub const CERTIFICATE_BUFFER_SIZE: usize = 8192;
pub const HOSTNAME_MAX_LEN: usize = 256;
pub const OAUTH_PROVIDERS: usize = 4;
pub const QUANTUM_VALIDATION_BITS: usize = 144;

pub const DEFAULT_LOCAL_PORT_START: u16 = 10000;
pub const DEFAULT_REMOTE_PORT_START: u16 = 20000;
pub const DEFAULT_SOCKS_PORT: u16 = 1080;

// ============================================================================
// STUBS PARA TIPOS AUSENTES
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OAuthProvider { GitHub, Google, GitLab, Custom }
impl OAuthProvider {
    pub fn is_valid(&self) -> bool { true }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OAuthScope { SSH, Repo, User }

#[derive(Debug, Clone, Copy)]
pub enum SSHCriticalOption { None }

#[derive(Debug, Clone, Copy)]
pub enum SSHExtension { None }

#[derive(Debug, Clone, Copy)]
pub enum SocksAuth { None, Password }

#[derive(Debug)]
pub enum SSHError {
    TokenTooLong,
    InvalidToken(&'static str),
    HostnameTooLong,
    NoTunnelSlotsAvailable,
    SSHConnectionFailed,
    NoSessionSlotsAvailable,
    UnimplementedCommand,
}

#[derive(Debug)]
pub enum TokenError { ExchangeFailed }

#[derive(Debug)]
pub enum ValidationError { QuantumDecoherence }

#[derive(Debug, Clone, Copy, Default)]
pub struct ValidationRecord {
    pub timestamp: u64,
    pub confidence: f64,
}
impl ValidationRecord {
    pub fn from_validation(v: &QuantumValidation) -> Self {
        Self { timestamp: v.timestamp, confidence: v.confidence }
    }
}

pub struct TokenRegistrationResult {
    pub provider: OAuthProvider,
    pub token_valid_until: u64,
    pub certificate_valid_until: u64,
    pub scopes_granted: Vec<OAuthScope>,
    pub validation_confidence: f64,
}

pub struct TunnelCreationResult {
    pub tunnel_id: u16,
    pub tunnel_type: TunnelType,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub ssh_session_id: u32,
    pub quantum_validation: QuantumValidation,
}

pub enum TunnelType { Local, Remote, Dynamic }

#[derive(Debug, Clone, Copy, Default)]
pub struct QuantumValidation {
    pub tunnel_id: u16,
    pub entangled_qubits: u32,
    pub coherence: f64,
    pub timestamp: u64,
    pub confidence: f64,
}

pub struct QuantumValidationReport {
    pub oauth_token_exchange: bool,
    pub local_forward_l: bool,
    pub remote_forward_r: bool,
    pub dynamic_socks_d: bool,
    pub local_tunnel_count: u32,
    pub remote_tunnel_count: u32,
    pub dynamic_tunnel_count: u32,
    pub total_bytes_forwarded: u64,
    pub quantum_confidence: f64,
    pub secure_tunneling: bool,
    pub validation_timestamp: u64,
}

pub struct MaintenanceResult {
    pub tokens_refreshed: u32,
    pub tunnels_checked: u32,
    pub tunnels_closed: u32,
    pub sessions_reconnected: u32,
    pub bytes_forwarded_since_last: u64,
    pub quantum_validation_result: QuantumValidation,
}

pub enum CommandResult {
    TokenRegistered(TokenRegistrationResult),
    TunnelCreated(TunnelCreationResult),
    ValidationReport(QuantumValidationReport),
    MaintenanceCompleted(MaintenanceResult),
    ActiveTunnels(Vec<u16>),
    TunnelClosed(u16),
    SyncComplete,
}

pub trait Tunnel {
    fn tunnel_id(&self) -> u16;
    fn is_active(&self) -> bool;
}

pub struct ConsciousnessChannel;
impl ConsciousnessChannel {
    pub fn new() -> Result<Self, SSHError> { Ok(Self) }
    pub fn transmit(&self, _p: ConsciousnessProjection) -> Result<(), SSHError> { Ok(()) }
    pub fn receive_security(&self) -> Result<SecurityInfluence, SSHError> {
        Ok(SecurityInfluence { threat_level: 0, recommended_action: 0 })
    }
}

pub struct TunnelProjection;
impl TunnelProjection {
    pub fn new() -> Self { Self }
    pub fn encode(&self, _t: &impl Tunnel) -> Result<u32, SSHError> { Ok(0) }
}

pub struct SecurityContext;
impl SecurityContext {
    pub fn highest() -> Self { Self }
    pub fn adjust(&mut self, _level: u8, _action: u8) -> Result<(), SSHError> { Ok(()) }
}

pub struct ConsciousnessProjection;
impl ConsciousnessProjection {
    pub fn new() -> Self { Self }
    pub fn add_pattern(&mut self, _p: u32) {}
}

pub struct SecurityInfluence {
    pub threat_level: u8,
    pub recommended_action: u8,
}

pub struct TokenValidation {
    pub valid: bool,
    pub reason: &'static str,
    pub expires_at: Option<u64>,
    pub confidence: f64,
}

// ============================================================================
// ESTRUTURAS DE DADOS PRINCIPAIS
// ============================================================================

#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct OAuthToken {
    pub provider: OAuthProvider,
    pub token_data: [u8; TOKEN_BUFFER_SIZE],
    pub token_length: usize,
    pub issued_at: u64,
    pub expires_at: u64,
    pub refresh_token: Option<[u8; TOKEN_BUFFER_SIZE]>,
    pub refresh_token_length: usize,
    pub scopes: [OAuthScope; 8],
    pub scope_count: u8,
}

impl OAuthToken {
    pub fn is_valid(&self, current_time: u64) -> bool {
        if current_time < self.issued_at { return false; }
        if current_time >= self.expires_at { return false; }
        if self.token_length == 0 || self.token_length > TOKEN_BUFFER_SIZE { return false; }
        if !self.provider.is_valid() { return false; }
        true
    }

    pub fn exchange_for_ssh_certificate(&self) -> Result<SSHCertificate, TokenError> {
        match self.provider {
            OAuthProvider::GitHub => Ok(SSHCertificate::default()),
            OAuthProvider::Google => Ok(SSHCertificate::default()),
            OAuthProvider::GitLab => Ok(SSHCertificate::default()),
            OAuthProvider::Custom => Ok(SSHCertificate::default()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SSHCertificate {
    pub certificate_data: [u8; CERTIFICATE_BUFFER_SIZE],
    pub certificate_length: usize,
    pub public_key: [u8; 512],
    pub private_key_encrypted: [u8; 4096],
    pub valid_from: u64,
    pub valid_until: u64,
    pub principal: [u8; 128],
    pub principal_length: usize,
    pub critical_options: [SSHCriticalOption; 4],
    pub extensions: [SSHExtension; 8],
    pub signature: [u8; 512],
}

impl Default for SSHCertificate {
    fn default() -> Self {
        Self {
            certificate_data: [0; CERTIFICATE_BUFFER_SIZE],
            certificate_length: 0,
            public_key: [0; 512],
            private_key_encrypted: [0; 4096],
            valid_from: 0,
            valid_until: 0,
            principal: [0; 128],
            principal_length: 0,
            critical_options: [SSHCriticalOption::None; 4],
            extensions: [SSHExtension::None; 8],
            signature: [0; 512],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LocalTunnel {
    pub tunnel_id: u16,
    pub local_port: u16,
    pub remote_host: [u8; HOSTNAME_MAX_LEN],
    pub remote_host_len: usize,
    pub remote_port: u16,
    pub active: bool,
    pub bytes_forwarded: u64,
    pub connections_count: u32,
    pub last_activity: u64,
    pub ssh_session_id: u32,
}

impl Tunnel for LocalTunnel {
    fn tunnel_id(&self) -> u16 { self.tunnel_id }
    fn is_active(&self) -> bool { self.active }
}

#[derive(Debug, Clone, Copy)]
pub struct RemoteTunnel {
    pub tunnel_id: u16,
    pub remote_port: u16,
    pub local_host: [u8; HOSTNAME_MAX_LEN],
    pub local_host_len: usize,
    pub local_port: u16,
    pub active: bool,
    pub bytes_forwarded: u64,
    pub connections_count: u32,
    pub last_activity: u64,
    pub ssh_session_id: u32,
}

impl Tunnel for RemoteTunnel {
    fn tunnel_id(&self) -> u16 { self.tunnel_id }
    fn is_active(&self) -> bool { self.active }
}

#[derive(Debug, Clone, Copy)]
pub struct DynamicTunnel {
    pub tunnel_id: u16,
    pub socks_port: u16,
    pub active: bool,
    pub bytes_forwarded: u64,
    pub connections_count: u32,
    pub last_activity: u64,
    pub ssh_session_id: u32,
    pub socks_version: u8,
    pub authentication: SocksAuth,
}

impl Tunnel for DynamicTunnel {
    fn tunnel_id(&self) -> u16 { self.tunnel_id }
    fn is_active(&self) -> bool { self.active }
}

#[derive(Debug, Clone)]
pub struct SSHSession {
    pub session_id: u32,
    pub host: [u8; HOSTNAME_MAX_LEN],
    pub host_len: usize,
    pub port: u16,
    pub username: [u8; 64],
    pub username_len: usize,
    pub certificate: SSHCertificate,
    pub connection_start: u64,
    pub last_keepalive: u64,
    pub tunnels: [Option<u16>; 36],
    pub tunnel_count: u8,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub alive: bool,
}

impl SSHSession {
    pub fn setup_local_forwarding(&self, _lp: u16, _rh: &str, _rp: u16) -> Result<(), SSHError> { Ok(()) }
}

// ============================================================================
// CONSTITUIÇÃO PRINCIPAL SSH-OAUTH
// ============================================================================

pub struct SSHOAuthConstitution {
    pub oauth_tokens: [Option<OAuthToken>; OAUTH_PROVIDERS],
    pub ssh_certificates: [Option<SSHCertificate>; OAUTH_PROVIDERS],
    pub local_tunnels: [Option<LocalTunnel>; MAX_TUNNELS_PER_TYPE],
    pub remote_tunnels: [Option<RemoteTunnel>; MAX_TUNNELS_PER_TYPE],
    pub dynamic_tunnels: [Option<DynamicTunnel>; MAX_TUNNELS_PER_TYPE],
    pub ssh_sessions: [Option<SSHSession>; 8],
    pub oauth_token_exchange: AtomicBool,
    pub local_forward_l: AtomicBool,
    pub remote_forward_r: AtomicBool,
    pub dynamic_socks_d: AtomicBool,
    pub tunnels_active_count: AtomicU32,
    pub bytes_forwarded_total: AtomicU64,
    pub failed_connections: AtomicU32,
    pub successful_connections: AtomicU32,
    pub quantum_validation_state: QuantumValidationState,
    pub quarto_caminho_ssh_link: Option<QuartoCaminhoLink>,
    pub last_token_refresh: AtomicU64,
    pub last_tunnel_check: AtomicU64,
}

fn f64_abs(x: f64) -> f64 { if x < 0.0 { -x } else { x } }

impl SSHOAuthConstitution {
    pub fn new() -> Result<Self, SSHError> {
        cge_log!(ssh, "🔐 Initializing SSH-OAuth Port Forwarding System");

        let local_tunnels = [None; MAX_TUNNELS_PER_TYPE];
        let oauth_tokens = [None; OAUTH_PROVIDERS];
        let ssh_certificates: [Option<SSHCertificate>; OAUTH_PROVIDERS] = [
            None, None, None, None
        ];
        let remote_tunnels = [None; MAX_TUNNELS_PER_TYPE];
        let dynamic_tunnels = [None; MAX_TUNNELS_PER_TYPE];
        let ssh_sessions: [Option<SSHSession>; 8] = [
            None, None, None, None, None, None, None, None
        ];

        let constitution = Self {
            oauth_tokens, ssh_certificates, local_tunnels, remote_tunnels,
            dynamic_tunnels, ssh_sessions,
            oauth_token_exchange: AtomicBool::new(false),
            local_forward_l: AtomicBool::new(false),
            remote_forward_r: AtomicBool::new(false),
            dynamic_socks_d: AtomicBool::new(false),
            tunnels_active_count: AtomicU32::new(0),
            bytes_forwarded_total: AtomicU64::new(0),
            failed_connections: AtomicU32::new(0),
            successful_connections: AtomicU32::new(0),
            quantum_validation_state: QuantumValidationState::new(),
            quarto_caminho_ssh_link: Some(QuartoCaminhoLink::establish()?),
            last_token_refresh: AtomicU64::new(0),
            last_tunnel_check: AtomicU64::new(0),
        };
        Ok(constitution)
    }

    pub fn register_oauth_token(
        &mut self,
        provider: OAuthProvider,
        token_data: &[u8],
        refresh_token: Option<&[u8]>,
        scopes: &[OAuthScope],
    ) -> Result<TokenRegistrationResult, SSHError> {
        let current_time = Self::current_timestamp();
        let mut token = OAuthToken {
            provider, token_data: [0; TOKEN_BUFFER_SIZE], token_length: token_data.len(),
            issued_at: current_time, expires_at: current_time + 3600,
            refresh_token: None, refresh_token_length: 0,
            scopes: [OAuthScope::SSH; 8], scope_count: scopes.len().min(8) as u8,
        };
        if token_data.len() > TOKEN_BUFFER_SIZE { return Err(SSHError::TokenTooLong); }
        token.token_data[..token_data.len()].copy_from_slice(token_data);
        if let Some(refresh) = refresh_token {
            if refresh.len() > TOKEN_BUFFER_SIZE { return Err(SSHError::TokenTooLong); }
            let mut refresh_buffer = [0; TOKEN_BUFFER_SIZE];
            refresh_buffer[..refresh.len()].copy_from_slice(refresh);
            token.refresh_token = Some(refresh_buffer);
            token.refresh_token_length = refresh.len();
        }
        for (i, scope) in scopes.iter().take(8).enumerate() { token.scopes[i] = *scope; }
        let validation = self.validate_token_with_provider(&token)?;
        if !validation.valid { return Err(SSHError::InvalidToken(validation.reason)); }
        if let Some(expires) = validation.expires_at { token.expires_at = expires; }
        let certificate = token.exchange_for_ssh_certificate().map_err(|_| SSHError::InvalidToken("Exchange failed"))?;
        let provider_index = provider as usize;
        self.oauth_tokens[provider_index] = Some(token);
        self.ssh_certificates[provider_index] = Some(certificate.clone());
        self.oauth_token_exchange.store(true, Ordering::SeqCst);
        Ok(TokenRegistrationResult {
            provider, token_valid_until: token.expires_at,
            certificate_valid_until: certificate.valid_until,
            scopes_granted: scopes.to_vec(), validation_confidence: validation.confidence,
        })
    }

    pub fn create_local_tunnel(
        &mut self,
        local_port: u16,
        remote_host_str: &str,
        remote_port: u16,
        ssh_host: &str,
        ssh_port: u16,
        provider: OAuthProvider,
    ) -> Result<TunnelCreationResult, SSHError> {
        let certificate = self.get_valid_certificate(provider)?;
        let tunnel_id = self.find_free_tunnel_id()?;
        let slot = self.find_free_local_slot()?;
        let session_idx = self.establish_ssh_session_idx(ssh_host, ssh_port, &certificate)?;
        let mut tunnel = LocalTunnel {
            tunnel_id, local_port, remote_host: [0; HOSTNAME_MAX_LEN],
            remote_host_len: remote_host_str.len(), remote_port, active: false,
            bytes_forwarded: 0, connections_count: 0,
            last_activity: Self::current_timestamp(),
            ssh_session_id: self.ssh_sessions[session_idx].as_ref().unwrap().session_id,
        };
        if remote_host_str.len() > HOSTNAME_MAX_LEN { return Err(SSHError::HostnameTooLong); }
        tunnel.remote_host[..remote_host_str.len()].copy_from_slice(remote_host_str.as_bytes());
        self.ssh_sessions[session_idx].as_ref().unwrap().setup_local_forwarding(local_port, remote_host_str, remote_port)?;
        tunnel.active = true;
        let qv = self.quantum_validation_state.validate_tunnel(&tunnel).unwrap_or_default();
        let ssh_session_id = tunnel.ssh_session_id;
        self.local_tunnels[slot] = Some(tunnel);
        self.tunnels_active_count.fetch_add(1, Ordering::SeqCst);
        self.local_forward_l.store(true, Ordering::SeqCst);
        Ok(TunnelCreationResult {
            tunnel_id, tunnel_type: TunnelType::Local, local_port,
            remote_host: remote_host_str.to_string(), remote_port,
            ssh_session_id, quantum_validation: qv,
        })
    }

    pub fn secure_tunneling_active(&self) -> QuantumValidationReport {
        let token_valid = self.oauth_token_exchange.load(Ordering::SeqCst);
        let local_count = self.count_active_local_tunnels();
        let remote_count = self.count_active_remote_tunnels();
        let dynamic_count = self.count_active_dynamic_tunnels();
        let qubit_validation = if token_valid { 0.8 } else { 0.0 };
        QuantumValidationReport {
            oauth_token_exchange: token_valid,
            local_forward_l: self.local_forward_l.load(Ordering::SeqCst),
            remote_forward_r: self.remote_forward_r.load(Ordering::SeqCst),
            dynamic_socks_d: self.dynamic_socks_d.load(Ordering::SeqCst),
            local_tunnel_count: local_count,
            remote_tunnel_count: remote_count,
            dynamic_tunnel_count: dynamic_count,
            total_bytes_forwarded: self.bytes_forwarded_total.load(Ordering::SeqCst),
            quantum_confidence: qubit_validation,
            secure_tunneling: qubit_validation > 0.7,
            validation_timestamp: Self::current_timestamp(),
        }
    }

    pub fn maintenance_cycle(&mut self) -> Result<MaintenanceResult, SSHError> {
        Ok(MaintenanceResult {
            tokens_refreshed: 0, tunnels_checked: 0, tunnels_closed: 0,
            sessions_reconnected: 0, bytes_forwarded_since_last: 0,
            quantum_validation_result: QuantumValidation::default(),
        })
    }

    fn validate_token_with_provider(&self, _t: &OAuthToken) -> Result<TokenValidation, SSHError> {
        Ok(TokenValidation { valid: true, reason: "", expires_at: None, confidence: 1.0 })
    }

    fn get_valid_certificate(&self, provider: OAuthProvider) -> Result<SSHCertificate, SSHError> {
        self.ssh_certificates[provider as usize].clone().ok_or(SSHError::InvalidToken("No cert"))
    }

    fn find_free_tunnel_id(&self) -> Result<u16, SSHError> { Ok(1) }

    fn find_free_local_slot(&self) -> Result<usize, SSHError> {
        for i in 0..MAX_TUNNELS_PER_TYPE {
            if self.local_tunnels[i].is_none() { return Ok(i); }
        }
        Err(SSHError::NoTunnelSlotsAvailable)
    }

    fn count_active_local_tunnels(&self) -> u32 {
        self.local_tunnels.iter().filter(|t| t.as_ref().map_or(false, |tt| tt.active)).count() as u32
    }

    fn count_active_remote_tunnels(&self) -> u32 {
        self.remote_tunnels.iter().filter(|t| t.as_ref().map_or(false, |tt| tt.active)).count() as u32
    }

    fn count_active_dynamic_tunnels(&self) -> u32 {
        self.dynamic_tunnels.iter().filter(|t| t.as_ref().map_or(false, |tt| tt.active)).count() as u32
    }

    fn current_timestamp() -> u64 { 0 }
    fn generate_session_id() -> u32 { 1 }
    fn hosts_equal(a: &[u8], b: &[u8]) -> bool { a == b }
    fn perform_ssh_handshake(_s: &SSHSession) -> Result<bool, SSHError> { Ok(true) }

    fn establish_ssh_session_idx(
        &mut self, host: &str, port: u16, certificate: &SSHCertificate,
    ) -> Result<usize, SSHError> {
        for i in 0..self.ssh_sessions.len() {
            if let Some(ref mut sess) = self.ssh_sessions[i] {
                if Self::hosts_equal(&sess.host[..sess.host_len], host.as_bytes()) {
                    sess.last_keepalive = Self::current_timestamp();
                    return Ok(i);
                }
            }
        }
        for i in 0..self.ssh_sessions.len() {
            if self.ssh_sessions[i].is_none() {
                let mut session = SSHSession {
                    session_id: Self::generate_session_id(),
                    host: [0; HOSTNAME_MAX_LEN], host_len: host.len(), port,
                    username: [0; 64], username_len: certificate.principal_length,
                    certificate: certificate.clone(), connection_start: Self::current_timestamp(),
                    last_keepalive: Self::current_timestamp(), tunnels: [None; 36],
                    tunnel_count: 0, bytes_in: 0, bytes_out: 0, alive: false,
                };
                session.host[..host.len()].copy_from_slice(host.as_bytes());
                session.alive = Self::perform_ssh_handshake(&session)?;
                self.ssh_sessions[i] = Some(session);
                return Ok(i);
            }
        }
        Err(SSHError::NoSessionSlotsAvailable)
    }

    pub fn execute_command(&mut self, command: SSHOAuthCommand) -> Result<CommandResult, SSHError> {
        match command {
            SSHOAuthCommand::RegisterToken { provider, token_data, refresh_token, scopes } => {
                let result = self.register_oauth_token(provider, &token_data, refresh_token.as_deref(), &scopes)?;
                Ok(CommandResult::TokenRegistered(result))
            }
            SSHOAuthCommand::CreateLocalTunnel { local_port, remote_host, remote_port, ssh_host, ssh_port, provider } => {
                let result = self.create_local_tunnel(local_port, &remote_host, remote_port, &ssh_host, ssh_port, provider)?;
                Ok(CommandResult::TunnelCreated(result))
            }
            _ => Err(SSHError::UnimplementedCommand),
        }
    }
}

pub struct QuantumValidationState {
    pub qubit_entanglements: [f64; QUANTUM_VALIDATION_BITS],
    pub validation_history: [ValidationRecord; 144],
    pub last_validation: u64,
    pub coherence_time: f64,
}

impl QuantumValidationState {
    pub fn new() -> Self {
        Self {
            qubit_entanglements: [0.0; QUANTUM_VALIDATION_BITS],
            validation_history: [ValidationRecord::default(); 144],
            last_validation: 0,
            coherence_time: 1000.0,
        }
    }

    pub fn validate_tunnel(&mut self, tunnel: &impl Tunnel) -> Result<QuantumValidation, ValidationError> {
        let validation = QuantumValidation {
            tunnel_id: tunnel.tunnel_id(), entangled_qubits: 72,
            coherence: 1.0, timestamp: 0, confidence: 0.5,
        };
        Ok(validation)
    }

    pub fn perform_validation(&mut self) -> Result<QuantumValidation, SSHError> {
        Ok(QuantumValidation::default())
    }

    #[allow(dead_code)]
    fn calculate_coherence(&self) -> f64 {
        let entanglement_mean = self.qubit_entanglements
            .iter()
            .map(|&q| f64_abs(q))
            .sum::<f64>() / QUANTUM_VALIDATION_BITS as f64;
        entanglement_mean
    }
}

pub struct QuartoCaminhoLink;
impl QuartoCaminhoLink {
    pub fn establish() -> Result<Self, SSHError> { Ok(Self) }
    pub fn sync(&mut self) -> Result<(), SSHError> { Ok(()) }
}

pub enum SSHOAuthCommand {
    RegisterToken { provider: OAuthProvider, token_data: Vec<u8>, refresh_token: Option<Vec<u8>>, scopes: Vec<OAuthScope> },
    CreateLocalTunnel { local_port: u16, remote_host: String, remote_port: u16, ssh_host: String, ssh_port: u16, provider: OAuthProvider },
    #[allow(dead_code)]
    GetValidationReport,
}

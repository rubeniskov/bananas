//! Tonic-served gRPC half of the engine. Hosted on
//! `BANANAS_ENGINE_GRPC_SOCKET` (default
//! `/run/bananas/engine-grpc.sock`), alongside the legacy
//! newline-JSON socket at `BANANAS_ENGINE_SOCKET`. Migration
//! between them is per-command: each Command variant moves to
//! gRPC at its own pace, and during the transition both paths
//! work.

use std::path::PathBuf;

use bananas_proto::engine::v1::{
    AuthenticateRequest, AuthenticateResponse, ChangeOwnPasswordRequest, ChangeOwnPasswordResponse,
    CreateUserRequest, CreateUserResponse, ExportUsersRequest, ExportUsersResponse,
    InstalledPackage as ProtoInstalledPackage, ListTimezonesRequest, ListTimezonesResponse,
    OpkgListInstalledRequest, OpkgListInstalledResponse, OpkgListUpgradableRequest,
    OpkgListUpgradableResponse, OpkgUpdateRequest, OpkgUpdateResponse, OpkgUpgradeRequest,
    OpkgUpgradeResponse, OpkgUpgradeStatusRequest, OpkgUpgradeStatusResponse,
    ReadServiceConfigRequest, ReadServiceConfigResponse, RebootSystemRequest, RebootSystemResponse,
    SetTimezoneRequest, SetTimezoneResponse, UpgradablePackage as ProtoUpgradablePackage,
    WriteExportsRequest, WriteExportsResponse, WriteFstabRequest, WriteFstabResponse,
    WriteServiceConfigRequest, WriteServiceConfigResponse,
    engine_service_server::{EngineService, EngineServiceServer},
};
use tonic::{Request, Response, Status};

use crate::Cx;
use crate::{
    authenticate, change_own_password, create_user, list_timezones_vec, list_users, opkg,
    read_service_config, reboot_system, set_timezone, verify_shadow_password, write_exports,
    write_fstab, write_service_config,
};

/// Engine gRPC service. Holds the same `Cx` (on-disk paths) the
/// newline-JSON dispatch threads through; passed by value so the
/// service is `Clone` friendly when tonic spawns per-request
/// futures.
pub struct EngineGrpc {
    pub cx: Cx,
}

#[tonic::async_trait]
impl EngineService for EngineGrpc {
    async fn authenticate(
        &self,
        req: Request<AuthenticateRequest>,
    ) -> Result<Response<AuthenticateResponse>, Status> {
        let body = req.into_inner();
        // Same flow as the newline-JSON `Command::Authenticate`:
        // verify the hash, check group membership, surface the
        // password_expired sentinel as a structured field. Errors
        // collapse into a single generic message — same redaction
        // policy as the legacy path.
        match authenticate(&body.username, &body.password, &self.cx.shadow).await {
            Ok(()) => Ok(Response::new(AuthenticateResponse {
                lastchg_zero: false,
            })),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("password_expired") {
                    // Authenticated, but password rotation forced.
                    return Ok(Response::new(AuthenticateResponse { lastchg_zero: true }));
                }
                tracing::warn!(user=%body.username, error=%msg, "auth failed (gRPC)");
                Err(Status::unauthenticated("invalid credentials"))
            }
        }
    }

    async fn change_own_password(
        &self,
        req: Request<ChangeOwnPasswordRequest>,
    ) -> Result<Response<ChangeOwnPasswordResponse>, Status> {
        let body = req.into_inner();
        // Same redaction as Authenticate — every failure surfaces
        // as a single Unauthenticated so callers can't distinguish
        // "user doesn't exist" from "old password wrong" from "new
        // password rejected by safety check".
        match change_own_password(
            &body.username,
            &body.old_password,
            &body.new_password,
            &self.cx.shadow,
        )
        .await
        {
            Ok(()) => Ok(Response::new(ChangeOwnPasswordResponse {})),
            Err(e) => {
                tracing::warn!(user=%body.username, error=%e, "change_own_password failed (gRPC)");
                Err(Status::unauthenticated("invalid credentials"))
            }
        }
    }

    async fn reboot_system(
        &self,
        _req: Request<RebootSystemRequest>,
    ) -> Result<Response<RebootSystemResponse>, Status> {
        // Distinct from auth: failure here is a real systemd
        // problem, not a security boundary, so we surface the
        // captured stderr for the UI to render.
        match reboot_system().await {
            Ok(output) => Ok(Response::new(RebootSystemResponse { output })),
            Err(e) => {
                tracing::warn!(error=%e, "reboot_system failed (gRPC)");
                Err(Status::internal(format!("reboot failed: {e}")))
            }
        }
    }

    async fn set_timezone(
        &self,
        req: Request<SetTimezoneRequest>,
    ) -> Result<Response<SetTimezoneResponse>, Status> {
        let body = req.into_inner();
        // Bad-tz strings are caller-supplied, so surface as
        // InvalidArgument; a timedatectl failure or persist error
        // is a real engine problem and maps to Internal.
        match set_timezone(&body.tz).await {
            Ok(output) => Ok(Response::new(SetTimezoneResponse { output })),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(tz = %body.tz, error = %msg, "set_timezone failed (gRPC)");
                if msg.starts_with("invalid timezone") || msg.starts_with("unknown timezone") {
                    Err(Status::invalid_argument(msg))
                } else {
                    Err(Status::internal(msg))
                }
            }
        }
    }

    async fn list_timezones(
        &self,
        _req: Request<ListTimezonesRequest>,
    ) -> Result<Response<ListTimezonesResponse>, Status> {
        match list_timezones_vec().await {
            Ok(zones) => Ok(Response::new(ListTimezonesResponse { zones })),
            Err(e) => {
                tracing::warn!(error = %e, "list_timezones failed (gRPC)");
                Err(Status::internal(format!("list_timezones failed: {e}")))
            }
        }
    }

    async fn read_service_config(
        &self,
        req: Request<ReadServiceConfigRequest>,
    ) -> Result<Response<ReadServiceConfigResponse>, Status> {
        let name = req.into_inner().name;
        // Unknown allowlist names are caller-controlled, so they
        // map to InvalidArgument; anything else is a real fs error.
        match read_service_config(&name).await {
            Ok(content) => Ok(Response::new(ReadServiceConfigResponse { content })),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(name = %name, error = %msg, "read_service_config failed (gRPC)");
                if msg.starts_with("unknown service config") {
                    Err(Status::invalid_argument(msg))
                } else {
                    Err(Status::internal(msg))
                }
            }
        }
    }

    async fn write_service_config(
        &self,
        req: Request<WriteServiceConfigRequest>,
    ) -> Result<Response<WriteServiceConfigResponse>, Status> {
        let body = req.into_inner();
        match write_service_config(&body.name, &body.content).await {
            Ok(output) => Ok(Response::new(WriteServiceConfigResponse { output })),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(name = %body.name, error = %msg, "write_service_config failed (gRPC)");
                if msg.starts_with("unknown service config") || msg.starts_with("invalid TOML") {
                    Err(Status::invalid_argument(msg))
                } else {
                    Err(Status::internal(msg))
                }
            }
        }
    }

    async fn opkg_update(
        &self,
        _req: Request<OpkgUpdateRequest>,
    ) -> Result<Response<OpkgUpdateResponse>, Status> {
        match opkg::update().await {
            Ok(output) => Ok(Response::new(OpkgUpdateResponse { output })),
            Err(e) => {
                tracing::warn!(error = %e, "opkg_update failed (gRPC)");
                Err(Status::internal(format!("opkg update: {e}")))
            }
        }
    }

    async fn opkg_list_upgradable(
        &self,
        _req: Request<OpkgListUpgradableRequest>,
    ) -> Result<Response<OpkgListUpgradableResponse>, Status> {
        match opkg::list_upgradable().await {
            Ok(rows) => Ok(Response::new(OpkgListUpgradableResponse {
                packages: rows
                    .into_iter()
                    .map(|p| ProtoUpgradablePackage {
                        name: p.name,
                        installed: p.installed,
                        candidate: p.candidate,
                    })
                    .collect(),
            })),
            Err(e) => {
                tracing::warn!(error = %e, "opkg_list_upgradable failed (gRPC)");
                Err(Status::internal(format!("opkg list-upgradable: {e}")))
            }
        }
    }

    async fn opkg_list_installed(
        &self,
        _req: Request<OpkgListInstalledRequest>,
    ) -> Result<Response<OpkgListInstalledResponse>, Status> {
        match opkg::list_installed().await {
            Ok(rows) => Ok(Response::new(OpkgListInstalledResponse {
                packages: rows
                    .into_iter()
                    .map(|p| ProtoInstalledPackage {
                        name: p.name,
                        version: p.version,
                    })
                    .collect(),
            })),
            Err(e) => {
                tracing::warn!(error = %e, "opkg_list_installed failed (gRPC)");
                Err(Status::internal(format!("opkg list-installed: {e}")))
            }
        }
    }

    async fn opkg_upgrade(
        &self,
        req: Request<OpkgUpgradeRequest>,
    ) -> Result<Response<OpkgUpgradeResponse>, Status> {
        let packages = req.into_inner().packages;
        match opkg::upgrade(&packages).await {
            Ok(output) => Ok(Response::new(OpkgUpgradeResponse { output })),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(error = %msg, "opkg_upgrade failed (gRPC)");
                // The engine's `upgrade()` rejects re-entry while a
                // unit is already active. Surface that as
                // FailedPrecondition so the webadmin can map it to
                // HTTP 409 Conflict.
                if msg.contains("already") {
                    Err(Status::failed_precondition(msg))
                } else {
                    Err(Status::internal(format!("opkg upgrade: {msg}")))
                }
            }
        }
    }

    async fn opkg_upgrade_status(
        &self,
        req: Request<OpkgUpgradeStatusRequest>,
    ) -> Result<Response<OpkgUpgradeStatusResponse>, Status> {
        let since = req.into_inner().since;
        match opkg::upgrade_status(since).await {
            Ok(s) => Ok(Response::new(OpkgUpgradeStatusResponse {
                state: s.state,
                log: s.log,
                log_offset: s.log_offset,
                exit_code: s.exit_code.unwrap_or(-1),
                has_exit_code: s.exit_code.is_some(),
            })),
            Err(e) => {
                tracing::warn!(error = %e, "opkg_upgrade_status failed (gRPC)");
                Err(Status::internal(format!("opkg upgrade-status: {e}")))
            }
        }
    }

    async fn write_exports(
        &self,
        req: Request<WriteExportsRequest>,
    ) -> Result<Response<WriteExportsResponse>, Status> {
        let content = req.into_inner().content;
        match write_exports(&content, &self.cx.exports).await {
            Ok(output) => Ok(Response::new(WriteExportsResponse { output })),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(error = %msg, "write_exports failed (gRPC)");
                // The validator surfaces every line-shape error
                // with a `line N:` prefix; treat those as
                // caller-supplied bad-input.
                if msg.contains("line ") || msg.starts_with("invalid") {
                    Err(Status::invalid_argument(msg))
                } else {
                    Err(Status::internal(msg))
                }
            }
        }
    }

    async fn write_fstab(
        &self,
        req: Request<WriteFstabRequest>,
    ) -> Result<Response<WriteFstabResponse>, Status> {
        let content = req.into_inner().content;
        match write_fstab(&content).await {
            Ok(output) => Ok(Response::new(WriteFstabResponse { output })),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(error = %msg, "write_fstab failed (gRPC)");
                if msg.contains("line ") || msg.starts_with("invalid") {
                    Err(Status::invalid_argument(msg))
                } else {
                    Err(Status::internal(msg))
                }
            }
        }
    }

    async fn export_users(
        &self,
        _req: Request<ExportUsersRequest>,
    ) -> Result<Response<ExportUsersResponse>, Status> {
        match list_users(true).await {
            Ok(users_json) => Ok(Response::new(ExportUsersResponse { users_json })),
            Err(e) => {
                tracing::warn!(error = %e, "export_users failed (gRPC)");
                Err(Status::internal(format!("export_users: {e}")))
            }
        }
    }

    async fn create_user(
        &self,
        req: Request<CreateUserRequest>,
    ) -> Result<Response<CreateUserResponse>, Status> {
        let body = req.into_inner();
        let full_name = if body.full_name.is_empty() {
            None
        } else {
            Some(body.full_name.as_str())
        };
        match create_user(
            &body.username,
            &body.password,
            full_name,
            body.admin,
            body.password_is_hash,
        )
        .await
        {
            Ok(()) => Ok(Response::new(CreateUserResponse {})),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!(user = %body.username, error = %msg, "create_user failed (gRPC)");
                // Caller-visible bad-input → InvalidArgument; an
                // already-existing user is FailedPrecondition so a
                // bundle restore can distinguish skipped from broken.
                if msg.starts_with("invalid") || msg.contains("password") {
                    Err(Status::invalid_argument(msg))
                } else if msg.contains("already exists") {
                    Err(Status::already_exists(msg))
                } else {
                    Err(Status::internal(msg))
                }
            }
        }
    }
}

/// Bind the gRPC Unix socket and serve the EngineService. Called
/// from `main` as a separate `tokio::spawn` so the legacy
/// newline-JSON listener continues to run on the original socket.
pub async fn serve(grpc_socket: PathBuf, cx: Cx) -> anyhow::Result<()> {
    if grpc_socket.exists() {
        let _ = std::fs::remove_file(&grpc_socket);
    }
    if let Some(parent) = grpc_socket.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = tokio::net::UnixListener::bind(&grpc_socket)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&grpc_socket, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket=%grpc_socket.display(), "engine gRPC listening");

    let svc = EngineGrpc { cx };
    let incoming = futures_util::stream::unfold(listener, |listener| async move {
        match listener.accept().await {
            Ok((stream, _addr)) => Some((Ok::<_, std::io::Error>(stream), listener)),
            Err(e) => {
                tracing::warn!(error=%e, "engine gRPC accept failed");
                Some((Err(e), listener))
            }
        }
    });

    tonic::transport::Server::builder()
        .add_service(EngineServiceServer::new(svc))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}

// silence unused-import warning when bananas-engine is built
// with both the lib and bin: `verify_shadow_password` is used
// only when the gRPC mount migrates additional RPCs.
#[allow(dead_code)]
fn _keep_used() {
    let _ = verify_shadow_password;
}

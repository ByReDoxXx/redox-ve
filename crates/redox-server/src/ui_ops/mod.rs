//! Konuk işlemlerinin istek işleyicileri (düzenleme, anlık görüntü, yedek, klon, konsol, toplu işlem).

#[allow(unused_imports)]
use crate::i18n::{tfmt, tx};
use axum::extract::{Path, Query, RawForm, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use maud::html;
use redox_core::{parse_forwards, parse_tags, Action, Error, GuestPatch, VmId};
use serde::Deserialize;

use crate::auth::Identity;
use crate::console;
use crate::state::{now, AppState};
use crate::ui::{capitalize, flash, Ctx, PageQuery};
use crate::view::Current;
use crate::view_ops::{self, Confirm, EditForm};

mod admin;
mod backups;
mod screen;
mod switches;
mod edit;
mod files;
mod hardware;
mod lifecycle;
mod mirrors;

pub use admin::*;
pub use backups::*;
pub use screen::*;
pub use switches::*;
pub use edit::*;
pub use files::*;
pub use hardware::*;
pub use lifecycle::*;
pub use mirrors::*;

fn vmid_of(raw: &str) -> Result<VmId, Redirect> {
    raw.parse().map_err(|_| flash("/", "error", &tfmt(format!("Geçersiz konuk kimliği: {raw}"))))
}

fn failed(path: &str, e: &Error) -> Redirect {
    match e {
        Error::GuestNotFound(_) => flash("/", "error", &capitalize(&e.to_string())),
        _ => flash(path, "error", &capitalize(&e.to_string())),
    }
}

//! Konuk işlemlerinin sayfaları: konsol, anlık görüntüler, yedekler, donanım, seçenekler, çoğaltma ve onay.

use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::i18n::{tfmt, tx};
use redox_core::{Guest, GuestKind, GuestStatus};

use crate::console::SPECIAL_KEYS;
use crate::view::{datetime, guest_label, guest_name, guest_state, icon, layout, page_head, FormOptions, Page};

mod backups;
mod clone;
mod console;
mod hardware;
mod options;

pub use backups::*;
pub use clone::*;
pub use console::*;
pub use hardware::*;
pub use options::*;

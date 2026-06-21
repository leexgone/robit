//! Extension keys for ToolContext.extensions map.
//!
//! Each key identifies a platform-specific extension that tools can access
//! via `ctx.extensions.get(key)` and downcast to the appropriate trait.

use std::any::Any;
use std::sync::Arc;

use crate::frontend::PlatformExt;

/// Extension keys for ToolContext.extensions.
pub mod keys {
    /// Platform file/media operations (upload, send).
    /// Value type: `Arc<PlatformExtWrapper>`
    pub const PLATFORM_EXT: &str = "chatbot.platform_ext";
}

/// Wrapper that makes `Arc<dyn PlatformExt>` storable in the `Any`-based
/// extensions map. Use `downcast_ref::<PlatformExtWrapper>()` to retrieve it.
pub struct PlatformExtWrapper(pub Arc<dyn PlatformExt>);

impl PlatformExtWrapper {
    /// Wrap a `PlatformExt` implementation for storage in ToolContext.extensions.
    pub fn new(ext: Arc<dyn PlatformExt>) -> Arc<dyn Any + Send + Sync> {
        Arc::new(Self(ext))
    }
}

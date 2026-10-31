/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Filtering of redundant `glActiveTexture` and `glBindTexture(GL_TEXTURE_2D)`
//! calls for the native OpenGL ES 1.1 backend.
//!
//! Apps often rebind the texture that is already bound, and on a real driver
//! every such call is still a call into the driver (and may mark state dirty).
//! [TextureBindingCache] mirrors the context's active texture unit and the
//! `GL_TEXTURE_2D` binding of each unit so [GLES1Native] can skip calls that
//! cannot change anything.
//!
//! The cache only ever answers "this call is certainly redundant". Whenever the
//! real state might differ from what was recorded, it forgets (the entry
//! becomes unknown) and the call is forwarded:
//!
//! - Any `glDeleteTextures` in any context invalidates every cache, because
//!   texture names are shared between contexts of a sharegroup and a deleted
//!   name may be reused, see [note_textures_deleted].
//! - An `glActiveTexture` for a unit that is not tracked (including invalid
//!   ones, which the driver rejects) makes the active unit unknown, and a bind
//!   while it is unknown forgets all bindings.
//!
//! This is deliberately not applied to other targets, buffers or `glEnable`
//! state; those are separate decisions with separate hazards.

use super::gles11_raw as gles11; // constants only
use super::gles11_raw::types::{GLenum, GLuint};
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bound on texture units tracked. Units beyond the driver's
/// `GL_MAX_TEXTURE_UNITS` (or this bound) are passed through uncached.
pub const MAX_TRACKED_UNITS: usize = 8;

/// Bumped on every `glDeleteTextures` made through any context.
static TEXTURE_DELETE_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Current value to pass to the [TextureBindingCache] methods.
pub fn texture_delete_epoch() -> u64 {
    TEXTURE_DELETE_EPOCH.load(Ordering::SeqCst)
}

/// Call this before forwarding any `glDeleteTextures`.
pub fn note_textures_deleted() {
    TEXTURE_DELETE_EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// Per-context mirror of the active texture unit and 2D texture bindings.
///
/// Methods return whether the call must be forwarded to the driver. They take
/// the current [texture_delete_epoch] as a parameter so they are deterministic.
#[derive(Debug)]
pub struct TextureBindingCache {
    max_units: usize,
    epoch: u64,
    /// Active unit (0-based), `None` if unknown.
    active: Option<usize>,
    /// `GL_TEXTURE_2D` binding per tracked unit, `None` if unknown.
    bound: [Option<GLuint>; MAX_TRACKED_UNITS],
}

impl TextureBindingCache {
    /// For a freshly created context, whose state is the GL default: unit 0
    /// active and texture 0 bound everywhere. `max_units` is the driver's
    /// `GL_MAX_TEXTURE_UNITS`.
    pub fn new(max_units: usize, epoch: u64) -> Self {
        let max_units = max_units.clamp(1, MAX_TRACKED_UNITS);
        let mut bound = [None; MAX_TRACKED_UNITS];
        for entry in bound.iter_mut().take(max_units) {
            *entry = Some(0);
        }
        Self {
            max_units,
            epoch,
            active: Some(0),
            bound,
        }
    }

    fn sync(&mut self, epoch: u64) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.bound = [None; MAX_TRACKED_UNITS];
        }
    }

    /// `glActiveTexture(texture)`. Returns true if it must be forwarded.
    pub fn active_texture(&mut self, texture: GLenum) -> bool {
        let unit = texture.wrapping_sub(gles11::TEXTURE0) as usize;
        if unit < self.max_units {
            if self.active == Some(unit) {
                return false;
            }
            self.active = Some(unit);
        } else {
            // Probably invalid, so the driver's active unit may not change,
            // but don't guess.
            self.active = None;
        }
        true
    }

    /// `glBindTexture(GL_TEXTURE_2D, texture)`. Returns true if it must be
    /// forwarded.
    pub fn bind_texture_2d(&mut self, epoch: u64, texture: GLuint) -> bool {
        self.sync(epoch);
        match self.active {
            Some(unit) => {
                if self.bound[unit] == Some(texture) {
                    false
                } else {
                    self.bound[unit] = Some(texture);
                    true
                }
            }
            None => {
                // The bind lands on a unit we can't name.
                self.bound = [None; MAX_TRACKED_UNITS];
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: GLenum = gles11::TEXTURE0;
    const E: u64 = 0;

    fn cache() -> TextureBindingCache {
        TextureBindingCache::new(2, E)
    }

    #[test]
    fn fresh_context_matches_gl_defaults() {
        let mut c = cache();
        assert!(!c.active_texture(T0), "unit 0 is already active");
        assert!(!c.bind_texture_2d(E, 0), "texture 0 is already bound");
        assert!(c.bind_texture_2d(E, 5));
    }

    #[test]
    fn repeated_bind_is_filtered_and_changes_are_forwarded() {
        let mut c = cache();
        assert!(c.bind_texture_2d(E, 7));
        assert!(!c.bind_texture_2d(E, 7));
        assert!(c.bind_texture_2d(E, 8));
        assert!(c.bind_texture_2d(E, 7));
    }

    #[test]
    fn bindings_are_per_unit() {
        let mut c = cache();
        assert!(c.bind_texture_2d(E, 3));
        assert!(c.active_texture(T0 + 1));
        assert!(c.bind_texture_2d(E, 3), "unit 1 has not bound 3 yet");
        assert!(!c.bind_texture_2d(E, 3));
        assert!(!c.active_texture(T0 + 1));
        assert!(c.active_texture(T0));
        assert!(!c.bind_texture_2d(E, 3), "unit 0 still has 3");
    }

    #[test]
    fn any_texture_deletion_invalidates_bindings_but_not_the_active_unit() {
        let mut c = cache();
        assert!(c.active_texture(T0 + 1));
        assert!(c.bind_texture_2d(E, 4));
        // A deletion (perhaps in another context) bumped the epoch.
        assert!(c.bind_texture_2d(E + 1, 4), "name 4 may be a new texture");
        assert!(!c.bind_texture_2d(E + 1, 4));
        assert!(!c.active_texture(T0 + 1), "deletion doesn't move the unit");
    }

    #[test]
    fn untracked_or_invalid_units_make_state_unknown() {
        let mut c = cache();
        assert!(c.bind_texture_2d(E, 4));
        assert!(c.active_texture(T0 + 5), "beyond max_units: forwarded");
        assert!(c.bind_texture_2d(E, 4), "unit unknown: forwarded");
        // Everything is forgotten, including unit 0's binding.
        assert!(c.active_texture(T0));
        assert!(c.bind_texture_2d(E, 4));
        assert!(!c.bind_texture_2d(E, 4));
        // Enumerants below TEXTURE0 wrap around and are also untracked.
        assert!(c.active_texture(T0 - 1));
        assert!(c.active_texture(T0), "unit was unknown, so forward");
    }

    #[test]
    fn max_units_is_clamped() {
        let c = TextureBindingCache::new(0, E);
        assert_eq!(c.max_units, 1);
        let c = TextureBindingCache::new(1000, E);
        assert_eq!(c.max_units, MAX_TRACKED_UNITS);
    }

    #[test]
    fn global_epoch_only_moves_forward_on_deletion() {
        let before = texture_delete_epoch();
        note_textures_deleted();
        assert!(texture_delete_epoch() > before);
    }
}

#!/usr/bin/env python3
"""Independently compile and test the development GLES texture cache without Cargo.

The generated Rust module supplies only generated GL enum/type names so
the actual src/gles/binding_cache.rs is compiled and tested, unmodified.
"""
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    development = Path(os.environ.get("UF_DEV_ROOT", "development")).resolve()
    cache = development / "src/gles/binding_cache.rs"
    if not cache.is_file():
        raise SystemExit(f"Missing development cache module: {cache}")
    code = r'''
mod gles {
    mod gles11_raw {
        pub const TEXTURE0: u32 = 0x84C0;
        pub mod types {
            pub type GLenum = u32;
            pub type GLuint = u32;
        }
    }
    #[path = "CACHE_PATH"]
    mod binding_cache;

    #[cfg(test)]
    mod qa_tests {
        use super::binding_cache::{TextureBindingCache, MAX_TRACKED_UNITS};
        use super::gles11_raw::TEXTURE0;

        #[test]
        fn context_caches_are_isolated() {
            let mut a = TextureBindingCache::new(2, 17);
            let mut b = TextureBindingCache::new(2, 17);
            assert!(a.bind_texture_2d(17, 42));
            assert!(!a.bind_texture_2d(17, 42));
            assert!(b.bind_texture_2d(17, 42));
            assert!(!b.bind_texture_2d(17, 42));
            assert!(!a.bind_texture_2d(17, 42));
        }

        #[test]
        fn deletion_epoch_invalidates_each_context_on_next_bind() {
            let mut a = TextureBindingCache::new(2, 17);
            let mut b = TextureBindingCache::new(2, 17);
            assert!(a.bind_texture_2d(17, 42));
            assert!(b.bind_texture_2d(17, 42));
            assert!(!a.bind_texture_2d(17, 42));
            assert!(!b.bind_texture_2d(17, 42));
            assert!(a.bind_texture_2d(18, 42));
            assert!(b.bind_texture_2d(18, 42));
        }

        #[test]
        fn up_to_eight_units_are_independently_tracked() {
            let mut c = TextureBindingCache::new(MAX_TRACKED_UNITS, 0);
            for unit in 0..MAX_TRACKED_UNITS {
                if unit != 0 {
                    assert!(c.active_texture(TEXTURE0 + unit as u32));
                }
                assert!(c.bind_texture_2d(0, 42 + unit as u32));
            }
            for unit in 0..MAX_TRACKED_UNITS {
                assert!(c.active_texture(TEXTURE0 + unit as u32));
                assert!(!c.bind_texture_2d(0, 42 + unit as u32));
            }
        }
    }
}
'''.replace("CACHE_PATH", str(cache).replace("\\", "\\\\"))
    with tempfile.TemporaryDirectory() as t:
        root = Path(t)
        harness = root / "gles_cache_qa.rs"
        harness.write_text(code, encoding="utf-8")
        executable = root / "gles_cache_qa"
        subprocess.run(
            ["rustc", "--edition", "2021", "--test", "-D", "warnings",
             str(harness), "-o", str(executable)], check=True
        )
        subprocess.run([str(executable), "--test-threads=1"], check=True)


if __name__ == "__main__":
    main()

//! Persistent settings.
//!
//! Each feature declares its own typed [`Setting`] next to the code that uses
//! it (e.g. `engine::SOUNDS_ON`, `diagnostics::LOG_ENABLED`), so adding a
//! setting never touches a shared struct:
//!
//! ```ignore
//! pub const VOLUME: Setting<f64> = Setting { key: "Volume", default: 0.75 };
//! let volume = store.get(&VOLUME);
//! store.set(&VOLUME, &0.5);
//! ```
//!
//! The app stores them in `NSUserDefaults` (the bundle's defaults domain);
//! tests use an in-memory backend.

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSString, NSUserDefaults};

/// A named setting with its default value.
#[derive(Clone, Copy, Debug)]
pub struct Setting<T: Value> {
    /// The defaults key. Never rename a shipped key: it would reset the setting.
    pub key: &'static str,
    pub default: T,
}

/// The kinds of value a backend stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Number,
    Text,
}

/// A stored value.
#[derive(Clone, Debug, PartialEq)]
pub enum Stored {
    Bool(bool),
    Number(f64),
    Text(String),
}

/// Types a setting can hold.
pub trait Value: Copy {
    type Owned;
    const KIND: Kind;
    fn to_owned_value(self) -> Self::Owned;
    fn decode(stored: Stored) -> Option<Self::Owned>;
    fn encode(value: &Self::Owned) -> Stored;
}

impl Value for bool {
    type Owned = bool;
    const KIND: Kind = Kind::Bool;
    fn to_owned_value(self) -> bool {
        self
    }
    fn decode(stored: Stored) -> Option<bool> {
        match stored {
            Stored::Bool(b) => Some(b),
            _ => None,
        }
    }
    fn encode(value: &bool) -> Stored {
        Stored::Bool(*value)
    }
}

impl Value for f64 {
    type Owned = f64;
    const KIND: Kind = Kind::Number;
    fn to_owned_value(self) -> f64 {
        self
    }
    fn decode(stored: Stored) -> Option<f64> {
        match stored {
            Stored::Number(n) if n.is_finite() => Some(n),
            _ => None,
        }
    }
    fn encode(value: &f64) -> Stored {
        Stored::Number(*value)
    }
}

impl Value for &'static str {
    type Owned = String;
    const KIND: Kind = Kind::Text;
    fn to_owned_value(self) -> String {
        self.to_owned()
    }
    fn decode(stored: Stored) -> Option<String> {
        match stored {
            Stored::Text(s) => Some(s),
            _ => None,
        }
    }
    fn encode(value: &String) -> Stored {
        Stored::Text(value.clone())
    }
}

/// Where settings live.
pub trait Backend {
    /// The value under `key`, if one of kind `kind` is stored there.
    fn get(&self, key: &str, kind: Kind) -> Option<Stored>;
    fn set(&self, key: &str, value: Stored);
}

/// Typed access to the settings.
pub struct Store {
    backend: Box<dyn Backend>,
}

impl Store {
    /// The app's settings, in `NSUserDefaults`.
    pub fn standard() -> Store {
        Store::with_backend(UserDefaults::standard())
    }

    /// Settings that live only in memory, for tests.
    #[cfg(test)]
    pub fn in_memory() -> Store {
        Store::with_backend(Memory::default())
    }

    pub fn with_backend(backend: impl Backend + 'static) -> Store {
        Store { backend: Box::new(backend) }
    }

    /// The saved value, or the default if none (or one of another type) was saved.
    pub fn get<T: Value>(&self, setting: &Setting<T>) -> T::Owned {
        self.backend
            .get(setting.key, T::KIND)
            .and_then(T::decode)
            .unwrap_or_else(|| setting.default.to_owned_value())
    }

    pub fn set<T: Value>(&self, setting: &Setting<T>, value: &T::Owned) {
        self.backend.set(setting.key, T::encode(value));
    }
}

/// An in-memory backend.
#[cfg(test)]
#[derive(Default)]
pub struct Memory {
    values: RefCell<HashMap<String, Stored>>,
}

#[cfg(test)]
impl Backend for Memory {
    fn get(&self, key: &str, kind: Kind) -> Option<Stored> {
        let value = self.values.borrow().get(key).cloned()?;
        let matches = matches!(
            (&value, kind),
            (Stored::Bool(_), Kind::Bool)
                | (Stored::Number(_), Kind::Number)
                | (Stored::Text(_), Kind::Text)
        );
        matches.then_some(value)
    }

    fn set(&self, key: &str, value: Stored) {
        self.values.borrow_mut().insert(key.to_owned(), value);
    }
}

/// `NSUserDefaults`: persists across relaunches and reboots.
pub struct UserDefaults {
    defaults: Retained<NSUserDefaults>,
}

impl UserDefaults {
    pub fn standard() -> UserDefaults {
        UserDefaults { defaults: NSUserDefaults::standardUserDefaults() }
    }

    /// A separate defaults domain, so tests never touch the real settings.
    #[cfg(test)]
    pub fn suite(name: &str) -> UserDefaults {
        use objc2::AnyThread;
        let defaults = NSUserDefaults::initWithSuiteName(
            NSUserDefaults::alloc(),
            Some(&NSString::from_str(name)),
        )
        .expect("valid suite name");
        UserDefaults { defaults }
    }

    /// Removes the suite's values and its preferences file, which
    /// `removePersistentDomainForName` alone leaves behind, empty. The file
    /// is written back by `cfprefsd` asynchronously, so the removal is
    /// flushed first; otherwise the empty file reappears after the delete.
    #[cfg(test)]
    pub fn remove_suite(name: &str) {
        NSUserDefaults::standardUserDefaults()
            .removePersistentDomainForName(&NSString::from_str(name));
        UserDefaults::suite(name).defaults.synchronize();
        if let Some(home) = std::env::var_os("HOME") {
            let file =
                std::path::Path::new(&home).join(format!("Library/Preferences/{name}.plist"));
            let _ = std::fs::remove_file(file);
        }
    }
}

/// A defaults suite of its own for one test,
/// `io.github.cavallinilorenzo.clicky.tests.<area>.<pid>`. Dropping it
/// removes the suite and its preferences file, also when the test panics.
#[cfg(test)]
pub struct TestSuite {
    name: String,
}

#[cfg(test)]
impl TestSuite {
    pub fn new(area: &str) -> TestSuite {
        TestSuite {
            name: format!("io.github.cavallinilorenzo.clicky.tests.{area}.{}", std::process::id()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// A store over the suite, as a fresh launch would open it.
    pub fn store(&self) -> Store {
        Store::with_backend(UserDefaults::suite(&self.name))
    }
}

#[cfg(test)]
impl Drop for TestSuite {
    fn drop(&mut self) {
        UserDefaults::remove_suite(&self.name);
    }
}

impl Backend for UserDefaults {
    fn get(&self, key: &str, kind: Kind) -> Option<Stored> {
        let key = NSString::from_str(key);
        self.defaults.objectForKey(&key)?;
        Some(match kind {
            Kind::Bool => Stored::Bool(self.defaults.boolForKey(&key)),
            Kind::Number => Stored::Number(self.defaults.doubleForKey(&key)),
            Kind::Text => Stored::Text(self.defaults.stringForKey(&key)?.to_string()),
        })
    }

    fn set(&self, key: &str, value: Stored) {
        let key = NSString::from_str(key);
        match value {
            Stored::Bool(b) => self.defaults.setBool_forKey(b, &key),
            Stored::Number(n) => self.defaults.setDouble_forKey(n, &key),
            Stored::Text(s) => {
                let value = NSString::from_str(&s);
                let object: &AnyObject = &value;
                // SAFETY: NSString is a property-list type, as NSUserDefaults requires.
                unsafe { self.defaults.setObject_forKey(Some(object), &key) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAG: Setting<bool> = Setting { key: "Flag", default: true };
    const LEVEL: Setting<f64> = Setting { key: "Level", default: 0.75 };
    const NAME: Setting<&str> = Setting { key: "Name", default: "brown" };

    #[test]
    fn unsaved_settings_read_their_defaults() {
        let store = Store::in_memory();
        assert!(store.get(&FLAG));
        assert_eq!(store.get(&LEVEL), 0.75);
        assert_eq!(store.get(&NAME), "brown");
    }

    #[test]
    fn saved_settings_read_back() {
        let store = Store::in_memory();
        store.set(&FLAG, &false);
        store.set(&LEVEL, &0.25);
        store.set(&NAME, &"blue".to_owned());
        assert!(!store.get(&FLAG));
        assert_eq!(store.get(&LEVEL), 0.25);
        assert_eq!(store.get(&NAME), "blue");
    }

    #[test]
    fn a_value_of_another_type_reads_as_the_default() {
        let store = Store::in_memory();
        store.set(&NAME, &"x".to_owned());
        let clash = Setting { key: "Name", default: false };
        assert!(!store.get(&clash));
        store.set(&LEVEL, &f64::NAN);
        assert_eq!(store.get(&LEVEL), 0.75, "non-finite numbers are rejected");
    }

    /// The real backend: a value saved by one store is seen by a fresh one,
    /// as after a relaunch.
    #[test]
    fn user_defaults_persist_across_stores() {
        let suite = TestSuite::new("store");
        let first = suite.store();
        assert!(first.get(&FLAG), "a fresh domain has no saved values");
        first.set(&FLAG, &false);
        first.set(&LEVEL, &0.5);
        first.set(&NAME, &"red".to_owned());
        let second = suite.store();
        assert!(!second.get(&FLAG));
        assert_eq!(second.get(&LEVEL), 0.5);
        assert_eq!(second.get(&NAME), "red");
    }

    /// The preferences file of `suite`.
    fn plist(suite: &str) -> std::path::PathBuf {
        let home = std::env::var_os("HOME").expect("HOME");
        std::path::Path::new(&home).join(format!("Library/Preferences/{suite}.plist"))
    }

    #[test]
    fn a_test_suite_leaves_no_file_behind() {
        let suite = TestSuite::new("cleanup");
        let name = suite.name().to_owned();
        let store = suite.store();
        store.set(&FLAG, &false);
        assert!(!suite.store().get(&FLAG));
        drop(store);
        drop(suite);
        assert!(!plist(&name).exists(), "{} is left behind", plist(&name).display());
        assert!(Store::with_backend(UserDefaults::suite(&name)).get(&FLAG), "the values are gone");
        UserDefaults::remove_suite(&name);
    }

    #[test]
    fn a_test_suite_is_removed_even_when_the_test_fails() {
        let name = std::sync::Mutex::new(String::new());
        let panicked = std::panic::catch_unwind(|| {
            let suite = TestSuite::new("panic");
            *name.lock().unwrap() = suite.name().to_owned();
            suite.store().set(&LEVEL, &0.1);
            panic!("a failed assertion");
        });
        assert!(panicked.is_err());
        let name = name.into_inner().unwrap();
        assert!(!plist(&name).exists(), "{} is left behind", plist(&name).display());
    }
}

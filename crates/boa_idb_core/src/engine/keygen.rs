//! Key generator with savepoint rollback support (§2.11).
//!
//! The key generator produces integer keys in the range [1, 2^53 - 1].
//! It supports rollback via a stack of saved values.

use crate::error::IdbError;

/// Maximum key value (2^53 - 1).
const MAX_KEY: f64 = 9_007_199_254_740_991.0;

/// Key generator for auto-increment stores.
#[derive(Debug)]
pub struct KeyGenerator {
    /// Current key value.
    current: f64,
    /// Stack of saved values for savepoint rollback.
    save_stack: Vec<f64>,
}

impl KeyGenerator {
    /// Creates a new key generator with the given initial value.
    pub fn new(initial: f64) -> Self {
        Self {
            current: initial,
            save_stack: Vec::new(),
        }
    }

    /// Returns the current key value without incrementing.
    pub fn current(&self) -> f64 {
        self.current
    }

    /// Generates a new key: returns the current value and increments by 1.
    ///
    /// The value `2^53 - 1` itself is still issuable; only values *above* it
    /// are rejected with `IdbError::Constraint`.
    pub fn generate(&mut self) -> Result<f64, IdbError> {
        if self.current > MAX_KEY {
            return Err(IdbError::Constraint(
                "Key generator overflow: maximum value reached".into(),
            ));
        }
        let key = self.current;
        self.current += 1.0;
        Ok(key)
    }

    /// Possibly updates the generator value if the given key is >= current.
    ///
    /// Used when a user-supplied key is stored to keep the generator ahead.
    pub fn possibly_update(&mut self, key: f64) {
        if key >= self.current {
            self.current = key.floor() + 1.0;
        }
    }

    /// Saves the current generator value for later rollback.
    pub fn begin_request(&mut self) {
        self.save_stack.push(self.current);
    }

    /// Commits the current request savepoint (discards the saved value).
    ///
    /// Returns `InvalidState` if there is no matching `begin_request` — an
    /// unbalanced savepoint stack indicates a driver bug, not a silent no-op.
    pub fn commit_request(&mut self) -> Result<(), IdbError> {
        if self.save_stack.pop().is_none() {
            return Err(IdbError::InvalidState(
                "KeyGenerator commit without matching begin".into(),
            ));
        }
        Ok(())
    }

    /// Rolls back the generator to the last saved value.
    ///
    /// Returns `InvalidState` if there is no matching `begin_request`.
    pub fn rollback_request(&mut self) -> Result<(), IdbError> {
        match self.save_stack.pop() {
            Some(saved) => {
                self.current = saved;
                Ok(())
            }
            None => Err(IdbError::InvalidState(
                "KeyGenerator rollback without matching begin".into(),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_increments() {
        let mut kg = KeyGenerator::new(1.0);
        assert_eq!(kg.generate().unwrap(), 1.0);
        assert_eq!(kg.generate().unwrap(), 2.0);
        assert_eq!(kg.generate().unwrap(), 3.0);
        assert_eq!(kg.current(), 4.0);
    }

    #[test]
    fn test_possibly_update() {
        let mut kg = KeyGenerator::new(1.0);
        kg.possibly_update(5.0);
        assert_eq!(kg.current(), 6.0);

        // Should not decrease
        kg.possibly_update(3.0);
        assert_eq!(kg.current(), 6.0);
    }

    #[test]
    fn test_savepoint_rollback() {
        let mut kg = KeyGenerator::new(1.0);
        kg.generate().unwrap(); // current = 2
        kg.begin_request();
        kg.generate().unwrap(); // current = 3
        kg.generate().unwrap(); // current = 4
        kg.rollback_request().unwrap();
        assert_eq!(kg.current(), 2.0);
    }

    #[test]
    fn test_savepoint_commit() {
        let mut kg = KeyGenerator::new(1.0);
        kg.begin_request();
        kg.generate().unwrap(); // current = 2
        kg.commit_request().unwrap();
        assert_eq!(kg.current(), 2.0);
    }

    #[test]
    fn test_overflow() {
        // 2^53 - 1 itself is still issuable...
        let mut kg = KeyGenerator::new(MAX_KEY);
        assert_eq!(kg.generate().unwrap(), MAX_KEY);
        // ...but the next value is not representable as a key.
        assert!(kg.generate().is_err());
    }

    #[test]
    fn test_unbalanced_commit_fails() {
        let mut kg = KeyGenerator::new(1.0);
        assert!(kg.commit_request().is_err());
    }

    #[test]
    fn test_unbalanced_rollback_fails() {
        let mut kg = KeyGenerator::new(1.0);
        assert!(kg.rollback_request().is_err());
    }
}

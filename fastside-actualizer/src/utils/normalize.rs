use std::collections::HashSet;

use fastside_shared::serde_types::Instance;

/// Normalize instances by removing duplicates and sorting them.
pub fn normalize_instances(instances: &[Instance]) -> Vec<Instance> {
    let set: HashSet<Instance> = instances.iter().cloned().collect();
    let mut vec: Vec<Instance> = set.into_iter().collect();
    vec.sort();
    vec
}

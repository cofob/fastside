use std::collections::HashSet;

use fastside_shared::serde_types::Instance;

pub fn normalize_instances(instances: &[Instance]) -> Vec<Instance> {
    let set: HashSet<Instance> = instances.iter().cloned().collect();
}

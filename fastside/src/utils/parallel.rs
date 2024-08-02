//! Parallelise tasks.
//!
//! This struct is used to easily spawn async tasks and limit the number of
//! concurrent futures.

use tokio::task::JoinHandle;

/// Parallelise tasks.
///
/// This struct is used to easily spawn async tasks and limit the number of
/// concurrent futures.
///
/// Tasks shoud have the same return type. Return values are not stored.
///
/// # Example
///
/// ```
/// use tokio::runtime::Runtime;
/// use crate::utils::parallel::Parallelise;
///
/// let rt = Runtime::new().unwrap();
/// rt.block_on(async {
///   // Limit to 10 concurrent tasks
///   let mut parallel = Parallelise::with_capacity(10);
///   for i in 0..20 {
///     parallel.push(tokio::spawn(async move {
///       println!("Task {} started", i);
///       tokio::time::sleep(std::time::Duration::from_millis(100)).await;
///       println!("Task {} finished", i);
///     })).await;
///   }
///   // Wait for all tasks to finish
///   parallel.wait().await;
/// })
/// ```
pub struct Parallelise<T> {
    tasks: Vec<JoinHandle<T>>,
    results: Vec<T>,
    max_tasks: usize,
}

impl<T> Parallelise<T> {
    /// Create a new Parallelise struct.
    ///
    /// The default capacity is 256.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new Parallelise struct.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of concurrent tasks.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            tasks: Vec::with_capacity(capacity),
            results: Vec::new(),
            max_tasks: capacity,
        }
    }

    /// Create a new Parallelise struct.
    ///
    /// The maximum number of concurrent tasks is the doubled number of CPUs.
    #[inline]
    pub fn with_cpus() -> Self {
        Self::with_capacity(num_cpus::get() * 2)
    }

    /// Push a new task to the set.
    ///
    /// If the set is full, this function will wait for one of the tasks to
    /// finish before adding the new task.
    pub async fn push(&mut self, task: JoinHandle<T>) {
        loop {
            // If set have less than max_tasks, we can add new task
            if self.tasks.len() < self.max_tasks {
                break;
            }
            // Find finished tasks and remove them
            for (j, task) in self.tasks.iter_mut().enumerate() {
                if task.is_finished() {
                    // Add result to the results
                    self.results.push(task.await.unwrap());
                    // Remove task from the set
                    self.tasks.remove(j);
                    break;
                }
            }
            // Check set again
            if self.tasks.len() < self.max_tasks {
                break;
            }
            // Sleep for 5ms to avoid busy waiting
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // Add task to the set
        self.tasks.push(task);
    }

    /// Wait for all tasks to finish.
    ///
    /// This function will wait for all tasks to finish before returning.
    pub async fn wait(mut self) -> Vec<T> {
        // Wait for all tasks to finish
        for task in self.tasks.drain(..) {
            self.results.push(task.await.unwrap());
        }
        // Return results
        self.results
    }
}

impl<T> Default for Parallelise<T> {
    fn default() -> Self {
        Self::with_capacity(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parallelise() {
        let mut parallel = Parallelise::with_capacity(10);
        for _ in 0..100 {
            parallel.push(tokio::spawn(async move {})).await;
        }
        parallel.wait().await;
    }
}

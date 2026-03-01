use std::collections::HashMap;

use crate::crawler::{CrawledInstance, CrawledService};

#[askama::filter_fn]
pub fn sort_crawled_instances(
    l: &[CrawledInstance],
    _env: &dyn askama::Values,
) -> askama::Result<Vec<CrawledInstance>> {
    let mut new = l.to_owned();
    new.sort_by(|a, b| a.status.as_isize().cmp(&b.status.as_isize()));
    Ok(new)
}

#[askama::filter_fn]
pub fn sort_crawled_services<'a>(
    l: &'a HashMap<String, CrawledService>,
    _env: &dyn askama::Values,
) -> askama::Result<Vec<(&'a String, &'a CrawledService)>> {
    let mut new = l.iter().collect::<Vec<_>>();
    new.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    Ok(new.to_owned())
}

use std::collections::HashMap;

use crate::crawler::{CrawledInstance, CrawledService};

pub fn sort_crawled_instances(l: &[CrawledInstance]) -> askama::Result<Vec<CrawledInstance>> {
    let mut new = l.to_owned();
    new.sort_by(|a, b| a.status.as_isize().cmp(&b.status.as_isize()));
    Ok(new)
}

pub fn sort_crawled_services(l: &HashMap<String, CrawledService>) -> askama::Result<Vec<(&String, &CrawledService)>> {
    let mut new = l.iter().collect::<Vec<_>>();
    new.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    Ok(new.to_owned())
}

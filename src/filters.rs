use crate::crawler::CrawledInstance;

pub fn sort_crawled_instances(l: &[CrawledInstance]) -> askama::Result<Vec<CrawledInstance>> {
    let mut new = l.to_owned();
    new.sort_by(|a, b| a.status.as_isize().cmp(&b.status.as_isize()));
    Ok(new)
}

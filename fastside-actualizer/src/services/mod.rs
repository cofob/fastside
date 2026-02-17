mod akademik;
mod breezewiki;
mod default;
mod fastside;
mod gothub;
mod invidious;
mod koub;
mod libreddit;
mod libredirect;
mod librex;
mod scribe;
mod searx;
mod searxng;
mod simplytranslate;
mod soprano;
mod tent;
mod translite;

use crate::types::ServiceUpdater;

pub use default::DefaultInstanceChecker;

/// Get a service updater by name.
pub fn get_service_updater(name: &str) -> Option<Box<dyn ServiceUpdater>> {
    match name {
        "searx" => Some(Box::new(searx::SearxUpdater::new())),
        "searxng" => Some(Box::new(searxng::SearxngUpdater::new())),
        "simplytranslate" => Some(Box::new(simplytranslate::SimplyTranslateUpdater::new())),
        "soprano" => Some(Box::new(soprano::SopranoUpdater::new())),
        "invidious" => Some(Box::new(invidious::InvidiousUpdater::new())),
        "scribe" => Some(Box::new(scribe::ScribeUpdater::new())),
        "libreddit" => Some(Box::new(libreddit::LibredditUpdater::new())),
        "breezewiki" => Some(Box::new(breezewiki::BreezewikiUpdater::new())),
        "librex" => Some(Box::new(librex::LibrexUpdater::new())),
        "gothub" => Some(Box::new(gothub::GothubUpdater::new())),
        "tent" => Some(Box::new(tent::TentUpdater::new())),
        "akademik" => Some(Box::new(akademik::AkademikUpdater::new())),
        "translite" => Some(Box::new(translite::TransLiteUpdater::new())),
        "koub" => Some(Box::new(koub::KoubUpdater::new())),
        "fastside" => Some(Box::new(fastside::FastsideUpdater::new())),
        _ if libredirect::LIBREDIRECT_SERVICES.contains(&name) => {
            Some(Box::new(libredirect::LibredirectUpdater::new(name)))
        }
        _ => None,
    }
}

/// Get an instance checker by name.
#[allow(clippy::match_single_binding)]
pub fn get_instance_checker(
    name: &str,
) -> Box<(dyn crate::types::InstanceChecker + Send + Sync + 'static)> {
    match name {
        _ => Box::new(DefaultInstanceChecker::new()),
    }
}

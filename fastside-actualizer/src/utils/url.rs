pub fn default_domain_scheme(domain: &str) -> String {
    match domain {
        _ if domain.ends_with(".onion") => "http://".to_string(),
        _ if domain.ends_with(".i2p") => "http://".to_string(),
        _ => "https://".to_string(),
    }
}

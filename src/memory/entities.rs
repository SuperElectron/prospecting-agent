use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityRef {
    Contact(Uuid),
    Company(String),
}

impl EntityRef {
    pub fn company(raw_domain: &str) -> Self {
        Self::Company(crate::domain::normalize_domain(raw_domain))
    }

    pub fn tag(&self) -> String {
        match self {
            Self::Contact(id) => format!("contact:{id}"),
            Self::Company(domain) => format!("company:{}", crate::domain::normalize_domain(domain)),
        }
    }

    pub fn parse(tag: &str) -> Option<Self> {
        let (kind, value) = tag.split_once(':')?;
        match kind {
            "contact" => Uuid::parse_str(value).ok().map(Self::Contact),
            "company" if !value.is_empty() => Some(Self::company(value)),
            _ => None,
        }
    }
}

impl std::fmt::Display for EntityRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        let contact = EntityRef::Contact(Uuid::new_v4());
        assert_eq!(EntityRef::parse(&contact.tag()), Some(contact));
        let company = EntityRef::Company("acme.io".into());
        assert_eq!(EntityRef::parse(&company.tag()), Some(company));
    }

    #[test]
    fn unknown_kinds_and_garbage_parse_to_none() {
        assert_eq!(EntityRef::parse("deal:123"), None);
        assert_eq!(EntityRef::parse("contact:not-a-uuid"), None);
        assert_eq!(EntityRef::parse("no-colon"), None);
        assert_eq!(EntityRef::parse("company:"), None);
    }

    #[test]
    fn company_constructor_normalizes_case_scheme_and_www() {
        let a = EntityRef::company("  https://www.Acme.IO/about  ");
        let b = EntityRef::company("acme.io");
        assert_eq!(a, b);
        assert_eq!(a.tag(), "company:acme.io");
    }

    #[test]
    fn mixed_case_variant_still_tags_normalized() {
        let raw = EntityRef::Company("Acme.IO".into());
        assert_eq!(raw.tag(), "company:acme.io");
    }
}

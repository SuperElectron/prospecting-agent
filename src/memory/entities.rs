use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityRef {
    Contact(Uuid),
    Company(String),
}

impl EntityRef {
    pub fn tag(&self) -> String {
        match self {
            Self::Contact(id) => format!("contact:{id}"),
            Self::Company(domain) => format!("company:{domain}"),
        }
    }

    pub fn parse(tag: &str) -> Option<Self> {
        let (kind, value) = tag.split_once(':')?;
        match kind {
            "contact" => Uuid::parse_str(value).ok().map(Self::Contact),
            "company" => Some(Self::Company(value.to_string())),
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
    }
}

//! Typed identifiers. Each kind is its own type, so a device ID can never be passed where a crew
//! member ID is expected. Generation happens in the storage and security crates (this crate does
//! no I/O); here IDs are only parsed, compared and formatted.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps an existing UUID.
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.hyphenated().fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

typed_id!(
    /// A crew member.
    CrewMemberId
);
typed_id!(
    /// A user account (crew or ground).
    UserId
);
typed_id!(
    /// A registered device.
    DeviceId
);
typed_id!(
    /// An alarm instance.
    AlarmId
);
typed_id!(
    /// An audit log entry.
    AuditEntryId
);
typed_id!(
    /// A login session.
    SessionId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse_and_display_canonically() {
        let s = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let id: DeviceId = s.parse().expect("valid UUID");
        assert_eq!(id.to_string(), s);
        assert!("not-a-uuid".parse::<CrewMemberId>().is_err());
    }

    #[test]
    fn ids_serialize_as_plain_strings() {
        let id: UserId = "67e55044-10b1-426f-9247-bb680e5fe0c8"
            .parse()
            .expect("valid");
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(json, "\"67e55044-10b1-426f-9247-bb680e5fe0c8\"");
    }
}

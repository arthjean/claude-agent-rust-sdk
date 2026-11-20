macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant, )+
            /// A value unknown to this version of the crate, kept verbatim.
            /// Only the crate builds it, from a spelling no known variant
            /// has, so two values are equal exactly when their wire
            /// spellings are.
            Unknown($crate::types::UnknownValue),
        }

        impl $name {
            /// Wire spellings known to this version of the crate.
            pub const KNOWN: &'static [&'static str] = &[$($wire),+];

            /// Returns the wire spelling of this value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $wire, )+
                    Self::Unknown(value) => value.as_str(),
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $( $wire => Self::$variant, )+
                    other => Self::Unknown($crate::types::UnknownValue::new(other.to_owned())),
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $( $wire => Self::$variant, )+
                    _ => Self::Unknown($crate::types::UnknownValue::new(value)),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                <String as serde::Deserialize>::deserialize(deserializer).map(Self::from)
            }
        }
    };
}

macro_rules! setters {
    () => {};
    (opt $field:ident: $ty:ty; $($rest:tt)*) => {
        #[doc = concat!("Sets `", stringify!($field), "`.")]
        #[must_use]
        pub fn $field(mut self, value: impl Into<$ty>) -> Self {
            self.$field = Some(value.into());
            self
        }
        setters!($($rest)*);
    };
    (val $field:ident: $ty:ty; $($rest:tt)*) => {
        #[doc = concat!("Sets `", stringify!($field), "`.")]
        #[must_use]
        pub fn $field(mut self, value: $ty) -> Self {
            self.$field = Some(value);
            self
        }
        setters!($($rest)*);
    };
    (list $field:ident: $ty:ty; $($rest:tt)*) => {
        #[doc = concat!("Sets `", stringify!($field), "`.")]
        #[must_use]
        pub fn $field(mut self, values: impl IntoIterator<Item = impl Into<$ty>>) -> Self {
            self.$field = Some(values.into_iter().map(Into::into).collect());
            self
        }
        setters!($($rest)*);
    };
}

pub(crate) use setters;
pub(crate) use wire_enum;

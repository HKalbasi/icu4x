// This file is part of ICU4X. For terms of use, please see the file
// called LICENSE at the top level of the ICU4X source tree
// (online at: https://github.com/unicode-org/icu4x/blob/main/LICENSE ).

//! Collection of traits for providers that support serializing or deserializing data.
//!
//! ## Deserializing
//!
//! Providers that involve a [`serde::Deserializer`] to produce data from an opaque source should
//! implement [`SerdeDeDataProvider`]. For example, `FsDataProvider` implements [`SerdeDeDataProvider`].
//!
//! [`SerdeDeDataProvider`] can be made into a trait object. It is used over FFI.
//!
//! ## Serializing
//!
//! Providers that have full type information should implement [`DataProvider`]`<dyn `[`SerdeSeDataStruct`]`>`.
//! Note that a provider like `FsDataProvider` cannot implement that trait, because type information
//! on the data structs is required in order to deserialize and then serialize them.
//!
//! [`DataProvider`]`<dyn `[`SerdeSeDataStruct`]`>` is used by data exporters such as `FilesystemExporter`.

use crate::error::Error;
use crate::prelude::*;
use crate::yoke::*;
use alloc::boxed::Box;
use alloc::rc::Rc;

use core::ops::Deref;
use yoke::trait_hack::YokeTraitHack;

/// An object that receives data from a Serde Deserializer.
///
/// Implemented by `Option<`[`DataPayload`]`>`.
pub trait SerdeDeDataReceiver {
    /// Receives a reference-counted byte buffer.
    ///
    /// Upon calling this function, the receiver sends byte buffer back to the caller as the first
    /// argument of `f1`. The caller should then map the byte buffer to an
    /// [`erased_serde::Deserializer`] and pass it back to the receiver via `f2`.
    ///
    /// # Examples
    ///
    /// Deserialize from a reference-counted buffer:
    ///
    /// ```
    /// use icu_provider::prelude::*;
    /// use icu_provider::hello_world::*;
    /// use icu_provider::serde::SerdeDeDataReceiver;
    /// use std::rc::Rc;
    ///
    /// let json_text = "{\"message\":\"Hello World\"}";
    /// let rc_buffer: Rc<[u8]> = json_text.as_bytes().into();
    /// let mut receiver: Option<DataPayload<HelloWorldV1Marker>> = None;
    /// receiver
    ///     .receive_rc_buffer(rc_buffer, |bytes, f2| {
    ///         let mut d = serde_json::Deserializer::from_slice(bytes);
    ///         f2(&mut erased_serde::Deserializer::erase(&mut d))
    ///     })
    ///     .expect("Well-formed data");
    /// let payload = receiver.expect("Data is present");
    ///
    /// assert_eq!(payload.get().message, "Hello World");
    /// ```
    fn receive_rc_buffer(
        &mut self,
        rc_buffer: Rc<[u8]>,
        f1: for<'de> fn(
            bytes: &'de [u8],
            f2: &mut dyn FnMut(&mut dyn erased_serde::Deserializer<'de>),
        ),
    ) -> Result<(), Error>;

    /// Receives a yoked byte buffer.
    ///
    /// This function has behavior identical to that of [`SerdeDeDataReceiver::receive_rc_buffer`].
    fn receive_yoked_buffer(
        &mut self,
        yoked_buffer: Yoke<&'static [u8], Rc<[u8]>>,
        f1: for<'de> fn(
            bytes: &'de [u8],
            f2: &mut dyn FnMut(&mut dyn erased_serde::Deserializer<'de>),
        ),
    ) -> Result<(), Error>;

    /// Receives a `&'static` byte buffer via an [`erased_serde::Deserializer`].
    ///
    /// Note: Since the purpose of this function is to handle zero-copy deserialization of static
    /// byte buffers, we want `Deserializer<'static>` as opposed to `DeserializeOwned`.
    ///
    /// # Examples
    ///
    /// Deserialize from a string to create static references:
    ///
    /// ```
    /// use icu_provider::prelude::*;
    /// use icu_provider::hello_world::*;
    /// use icu_provider::serde::SerdeDeDataReceiver;
    ///
    /// let json_text = "{\"message\":\"Hello World\"}";
    /// let deserializer = &mut serde_json::Deserializer::from_str(json_text);
    /// let mut receiver: Option<DataPayload<HelloWorldV1Marker>> = None;
    /// receiver
    ///     .receive_static(&mut erased_serde::Deserializer::erase(deserializer))
    ///     .expect("Well-formed data");
    /// let payload = receiver.expect("Data is present");
    ///
    /// assert_eq!(payload.get().message, "Hello World");
    /// ```
    fn receive_static(
        &mut self,
        deserializer: &mut dyn erased_serde::Deserializer<'static>,
    ) -> Result<(), Error>;
}

impl<M> SerdeDeDataReceiver for Option<DataPayload<M>>
where
    M: DataMarker,
    M::Yokeable: serde::de::Deserialize<'static>,
    // Actual bound:
    //     for<'de> <M::Yokeable as Yokeable<'de>>::Output: serde::de::Deserialize<'de>,
    // Necessary workaround bound (see `yoke::trait_hack` docs):
    for<'de> YokeTraitHack<<M::Yokeable as Yokeable<'de>>::Output>: serde::de::Deserialize<'de>,
{
    fn receive_rc_buffer(
        &mut self,
        rc_buffer: Rc<[u8]>,
        f1: for<'de> fn(
            bytes: &'de [u8],
            f2: &mut dyn FnMut(&mut dyn erased_serde::Deserializer<'de>),
        ),
    ) -> Result<(), Error> {
        self.replace(DataPayload::try_from_rc_buffer(rc_buffer, move |bytes| {
            let mut holder = None;
            f1(bytes, &mut |deserializer| {
                holder.replace(
                    erased_serde::deserialize::<YokeTraitHack<<M::Yokeable as Yokeable>::Output>>(
                        deserializer,
                    )
                    .map(|w| w.0),
                );
            });
            // The holder is guaranteed to be populated so long as the lambda function was invoked,
            // which is in the contract of `receive_rc_buffer`.
            holder.unwrap()
        })?);
        Ok(())
    }

    fn receive_yoked_buffer(
        &mut self,
        yoked_buffer: Yoke<&'static [u8], Rc<[u8]>>,
        f1: for<'de> fn(
            bytes: &'de [u8],
            f2: &mut dyn FnMut(&mut dyn erased_serde::Deserializer<'de>),
        ),
    ) -> Result<(), Error> {
        self.replace(DataPayload::try_from_yoked_buffer(
            yoked_buffer,
            f1,
            move |bytes, f1, _| {
                let mut holder = None;
                f1(bytes, &mut |deserializer| {
                    holder.replace(
                    erased_serde::deserialize::<YokeTraitHack<<M::Yokeable as Yokeable>::Output>>(
                        deserializer,
                    )
                    .map(|w| w.0),
                );
                });
                // The holder is guaranteed to be populated so long as the lambda function was invoked,
                // which is in the contract of `receive_rc_buffer`.
                holder.unwrap()
            },
        )?);
        Ok(())
    }

    fn receive_static(
        &mut self,
        deserializer: &mut dyn erased_serde::Deserializer<'static>,
    ) -> Result<(), Error> {
        let obj: M::Yokeable = erased_serde::deserialize(deserializer)?;
        self.replace(DataPayload::from_owned(obj));
        Ok(())
    }
}

/// A type-erased data provider that loads payloads from a Serde Deserializer.
///
/// Uses [`erased_serde`] to allow the trait to be object-safe.
pub trait SerdeDeDataProvider {
    /// Query the provider for data, loading it into a [`SerdeDeDataReceiver`].
    ///
    /// Returns Ok if the request successfully loaded data. If data failed to load, returns an
    /// Error with more information.
    fn load_to_receiver(
        &self,
        req: &DataRequest,
        receiver: &mut dyn SerdeDeDataReceiver,
    ) -> Result<DataResponseMetadata, Error>;
}

/// Note: This impl returns `'static` payloads because borrowing is handled by [`Yoke`].
impl<M> DataProvider<M> for dyn SerdeDeDataProvider + 'static
where
    M: DataMarker,
    M::Yokeable: serde::de::Deserialize<'static>,
    // Actual bound:
    //     for<'de> <M::Yokeable as Yokeable<'de>>::Output: serde::de::Deserialize<'de>,
    // Necessary workaround bound (see `yoke::trait_hack` docs):
    for<'de> YokeTraitHack<<M::Yokeable as Yokeable<'de>>::Output>: serde::de::Deserialize<'de>,
{
    /// Serve objects implementing [`serde::Deserialize<'de>`] from a [`SerdeDeDataProvider`].
    fn load_payload(&self, req: &DataRequest) -> Result<DataResponse<M>, Error> {
        let mut payload = None;
        let metadata = self.load_to_receiver(req, &mut payload)?;
        Ok(DataResponse { metadata, payload })
    }
}

/// Auto-implemented trait for all data structs that support [`serde::Serialize`]. This trait is
/// usually used as a trait object in [`DataProvider`]`<dyn `[`SerdeSeDataStruct`]`>`.
pub trait SerdeSeDataStruct: 'static {
    /// Return this trait object reference for Serde serialization.
    ///
    /// # Examples
    ///
    /// ```
    /// use icu_provider::serde::SerdeSeDataStruct;
    /// use icu_provider::hello_world::HelloWorldV1;
    ///
    /// // Create type-erased reference
    /// let data = HelloWorldV1::default();
    /// let erased: &dyn SerdeSeDataStruct = &data;
    ///
    /// // Borrow as serialize trait object
    /// let serialize: &dyn erased_serde::Serialize = erased.as_serialize();
    ///
    /// // Serialize the object to a JSON string
    /// let mut buffer: Vec<u8> = vec![];
    /// serialize.erased_serialize(
    ///     &mut erased_serde::Serializer::erase(
    ///         &mut serde_json::Serializer::new(&mut buffer)
    ///     )
    /// ).expect("Serialization should succeed");
    /// assert_eq!("{\"message\":\"(und) Hello World\"}".as_bytes(), buffer);
    /// ```
    fn as_serialize(&self) -> &dyn erased_serde::Serialize;
}

impl<T> SerdeSeDataStruct for T
where
    T: 'static + serde::Serialize,
{
    fn as_serialize(&self) -> &dyn erased_serde::Serialize {
        self
    }
}

/// A wrapper around `Box<`[`SerdeSeDataStruct`]`>` for integration with DataProvider.
#[derive(yoke::Yokeable)]
pub struct SerdeSeDataStructBox(Box<dyn SerdeSeDataStruct>);

impl Deref for SerdeSeDataStructBox {
    type Target = dyn SerdeSeDataStruct;
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<M> crate::dynutil::UpcastDataPayload<M> for SerdeSeDataStructMarker
where
    M: DataMarker,
    for<'a> &'a <M::Yokeable as Yokeable<'a>>::Output: serde::Serialize,
{
    fn upcast(other: DataPayload<M>) -> DataPayload<SerdeSeDataStructMarker> {
        use crate::data_provider::DataPayloadInner;
        let b = match other.inner {
            DataPayloadInner::Owned(yoke) => Box::new(yoke) as Box<dyn SerdeSeDataStruct>,
            DataPayloadInner::RcBuf(yoke) => Box::new(yoke) as Box<dyn SerdeSeDataStruct>,
        };
        DataPayload::from_owned(SerdeSeDataStructBox(b))
    }
}

/// Marker type for [`SerdeSeDataStruct`].
pub struct SerdeSeDataStructMarker {}

impl DataMarker for SerdeSeDataStructMarker {
    type Yokeable = SerdeSeDataStructBox;
}
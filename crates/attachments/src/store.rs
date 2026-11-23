//! The attachment store: the one front door. It owns the layout, the limits and the bound on image work, and
//! offers the operations the rest of the app needs: save, read, look up a path, make a request version.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use vivianne_core::attachment::{FileRef, ImageRef};

use crate::error::{Code, Error, Result, fail};
use crate::files::{FileChunks, read_file_stream, save_file, save_file_stream};
use crate::image::normalize::Policy;
use crate::image::request::{RequestImage, Target, request_version, validate_target, variant_id};
use crate::layout::Layout;
use crate::limiter::{Cancel, Limiter};
use crate::prepare::{
    Limits, Prepared, SaveImage, StoredImage, commit_prepared, prepare_image, read_image,
};

/// Most simultaneous image transformations a store may be set to.
pub const MAX_COMPRESSION_CONCURRENCY: usize = 8;
/// A conservative default: image work is CPU heavy and the app has other things to draw.
pub const DEFAULT_COMPRESSION_CONCURRENCY: usize = 2;
/// Largest file stored when no other limit is set (R3.7: every saved file has a cap).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// Everything a store can be configured with. The default of each field is the documented one.
#[derive(Debug, Clone)]
pub struct Config {
    pub limits: Limits,
    pub policy: Policy,
    /// Simultaneous normalizations or request transformations, 1 through 8.
    pub compression_concurrency: usize,
    /// Largest stored file; `None` stores any size.
    pub max_file_bytes: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            policy: Policy::default(),
            compression_concurrency: DEFAULT_COMPRESSION_CONCURRENCY,
            max_file_bytes: Some(DEFAULT_MAX_FILE_BYTES),
        }
    }
}

/// One shared request transformation and the callers waiting for it.
#[derive(Debug, Default)]
struct Flight {
    result: Mutex<Option<Result<RequestImage>>>,
    done: Condvar,
    cancel: Cancel,
    waiters: AtomicUsize,
}

#[derive(Debug)]
pub struct Store {
    layout: Layout,
    config: Config,
    limiter: Limiter,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
}

impl Store {
    /// A store under `home`. Nothing is created on disk until something is saved.
    pub fn new(home: &std::path::Path, config: Config) -> std::result::Result<Self, String> {
        if !(1..=MAX_COMPRESSION_CONCURRENCY).contains(&config.compression_concurrency) {
            return Err(format!(
                "attachments: compression concurrency must be from 1 through {MAX_COMPRESSION_CONCURRENCY}"
            ));
        }
        Ok(Self {
            layout: Layout::new(home),
            limiter: Limiter::new(config.compression_concurrency),
            config,
            flights: Mutex::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.config.limits
    }

    #[must_use]
    pub fn policy(&self) -> &Policy {
        &self.config.policy
    }

    /// Checks one batch against the count, byte and type limits, before any member is decoded.
    pub fn check_batch(
        &self,
        count: usize,
        bytes: u64,
        types: impl IntoIterator<Item = vivianne_core::attachment::MediaType>,
    ) -> Result<()> {
        let limits = &self.config.limits;
        if count > limits.max_images_per_message {
            return fail(
                Code::TooManyImages,
                "Image batch exceeds the configured image-count limit.",
            );
        }
        if bytes > limits.max_message_image_bytes {
            return fail(
                Code::ImagesTooLarge,
                "Image batch exceeds the configured aggregate image-byte limit.",
            );
        }
        for media_type in types {
            if !limits.media_types.contains(&media_type) {
                return fail(
                    Code::UnsupportedImageType,
                    format!(
                        "Image type {} is not accepted by this deployment.",
                        media_type.mime()
                    ),
                );
            }
        }
        Ok(())
    }

    fn prepare(&self, input: &SaveImage) -> Result<Prepared> {
        self.limiter
            .run(|| prepare_image(input, &self.config.limits, &self.config.policy))
    }

    /// Runs the whole admission policy for one image without storing it.
    pub fn validate_image(&self, input: &SaveImage) -> Result<()> {
        self.prepare(input).map(|_| ())
    }

    /// Stores one image.
    pub fn save_image(&self, input: &SaveImage) -> Result<ImageRef> {
        commit_prepared(&self.layout, &self.prepare(input)?)
    }

    /// Stores an ordered batch: the batch limits are checked first, every member is prepared before any is written,
    /// and the references come back in input order. A refused member starts no writes.
    pub fn save_images(&self, inputs: &[SaveImage]) -> Result<Vec<ImageRef>> {
        let bytes = inputs.iter().map(|i| i.data.len() as u64).sum();
        self.check_batch(inputs.len(), bytes, inputs.iter().map(|i| i.media_type))?;
        let prepared = std::thread::scope(|scope| {
            let workers: Vec<_> = inputs
                .iter()
                .map(|input| scope.spawn(|| self.prepare(input)))
                .collect();
            workers
                .into_iter()
                .map(|w| {
                    w.join().unwrap_or_else(|_| {
                        fail(Code::AttachmentWriteFailed, "Image preparation failed.")
                    })
                })
                .collect::<Result<Vec<_>>>()
        })?;
        prepared
            .iter()
            .map(|p| commit_prepared(&self.layout, p))
            .collect()
    }

    /// Reads a stored image, verified against its reference.
    pub fn read_image(&self, image: &ImageRef, cancel: &Cancel) -> Result<StoredImage> {
        read_image(&self.layout, image, cancel)
    }

    /// Where the normalized object is on this machine, for a tool that opens it by path.
    pub fn image_host_path(&self, image: &ImageRef) -> Result<PathBuf> {
        self.layout.image_path(image)
    }

    /// Stores a file byte for byte.
    pub fn save_file(&self, data: &[u8], name: Option<&str>) -> Result<FileRef> {
        save_file(&self.layout, data, name, self.config.max_file_bytes)
    }

    /// Stores a file from a reader, counting its bytes as they come.
    pub fn save_file_stream(
        &self,
        reader: &mut dyn Read,
        name: Option<&str>,
        cancel: &Cancel,
    ) -> Result<FileRef> {
        save_file_stream(
            &self.layout,
            reader,
            name,
            self.config.max_file_bytes,
            cancel,
        )
    }

    /// Reads a stored file in verified chunks.
    pub fn read_file_stream(&self, file: &FileRef, cancel: &Cancel) -> Result<FileChunks> {
        read_file_stream(&self.layout, file, cancel)
    }

    /// Where the file is on this machine, under its display name.
    pub fn file_host_path(&self, file: &FileRef) -> Result<PathBuf> {
        self.layout.file_path(file)
    }

    /// The version of a stored image sized for one route. Callers that ask for the same version at the same time
    /// share one transformation; the work stops only when the last of them cancels.
    pub fn read_image_request(
        self: &Arc<Self>,
        image: &ImageRef,
        target: &Target,
        cancel: &Cancel,
    ) -> Result<RequestImage> {
        cancel.check()?;
        validate_target(target)?;
        let key = variant_id(&image.attachment_id, target);
        let flight = self.join_flight(&key, image, target);
        self.wait_for(&key, &flight, cancel)
    }

    fn join_flight(self: &Arc<Self>, key: &str, image: &ImageRef, target: &Target) -> Arc<Flight> {
        let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(flight) = flights.get(key).filter(|f| !f.cancel.is_cancelled()) {
            flight.waiters.fetch_add(1, Ordering::SeqCst);
            return flight.clone();
        }
        let flight = Arc::new(Flight::default());
        flight.waiters.store(1, Ordering::SeqCst);
        flights.insert(key.to_owned(), flight.clone());
        drop(flights);
        let (store, worker, image, target, key) = (
            self.clone(),
            flight.clone(),
            image.clone(),
            *target,
            key.to_owned(),
        );
        std::thread::spawn(move || {
            // A panic in image work must still settle the flight, or its waiters would wait for ever.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                store.limiter.run(|| {
                    let stored = read_image(&store.layout, &image, &worker.cancel)?;
                    request_version(
                        |hash| store.layout.request_image_path(hash),
                        &stored.data,
                        &image,
                        &target,
                        &worker.cancel,
                    )
                })
            }))
            .unwrap_or_else(|_| fail(Code::AttachmentWriteFailed, "Image transformation failed."));
            *worker.result.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            worker.done.notify_all();
            let mut flights = store.flights.lock().unwrap_or_else(PoisonError::into_inner);
            if flights.get(&key).is_some_and(|f| Arc::ptr_eq(f, &worker)) {
                flights.remove(&key);
            }
        });
        flight
    }

    fn wait_for(&self, key: &str, flight: &Arc<Flight>, cancel: &Cancel) -> Result<RequestImage> {
        let mut result = flight.result.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = result.as_ref() {
                flight.waiters.fetch_sub(1, Ordering::SeqCst);
                return outcome.clone();
            }
            if cancel.is_cancelled() {
                // The last waiter to leave stops the shared work.
                if flight.waiters.fetch_sub(1, Ordering::SeqCst) == 1 {
                    flight.cancel.cancel();
                    let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
                    if flights.get(key).is_some_and(|f| Arc::ptr_eq(f, flight)) {
                        flights.remove(key);
                    }
                }
                return Err(Error::Cancelled);
            }
            result = flight
                .done
                .wait_timeout(result, Duration::from_millis(20))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

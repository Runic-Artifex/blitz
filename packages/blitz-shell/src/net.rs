// use std::sync::Arc;

// use blitz_traits::net::NetWaker;
// use winit::event_loop::EventLoopProxy;

// use crate::{BlitzShellEvent, event::BlitzShellProxy};

// /// A NetWaker that wakes up our winit event loop
// pub struct BlitzShellNetWaker(BlitzShellProxy);

// impl BlitzShellNetWaker {
//     pub fn new(proxy: BlitzShellProxy) -> Self {
//         Self(proxy)
//     }

//     pub fn shared(proxy: BlitzShellProxy) -> Arc<dyn NetWaker> {
//         Arc::new(Self(proxy))
//     }
// }
// impl NetWaker for BlitzShellNetWaker {
//     fn wake(&self, doc_id: usize) {
//         self.0.send_event(BlitzShellEvent::RequestRedraw { doc_id })
//     }
// }

#[cfg(feature = "data-uri")]
mod data_uri_net_provider {
    //! Data-URI only networking for Blitz
    //!
    //! Provides an implementation of the [`blitz_traits::net::NetProvider`] trait.

    use blitz_traits::net::{Bytes, NetHandler, NetProvider, NetWaker, Request};
    use data_url::DataUrl;
    use std::sync::Arc;

    pub struct DataUriNetProvider {
        #[allow(unused)]
        waker: Option<Arc<dyn NetWaker>>,
    }
    impl DataUriNetProvider {
        pub fn new(waker: Option<Arc<dyn NetWaker>>) -> Self {
            Self { waker }
        }
        pub fn shared(waker: Option<Arc<dyn NetWaker>>) -> Arc<dyn NetProvider> {
            Arc::new(Self::new(waker))
        }
    }

    impl NetProvider for DataUriNetProvider {
        fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
            let url = request.url.to_string();
            match decode(&request) {
                Ok(bytes) => handler.bytes(url, Bytes::from(bytes)),
                Err(message) => handler.error(url, String::from(message)),
            }
        }
    }

    fn decode(request: &Request) -> Result<Vec<u8>, &'static str> {
        if request.url.scheme() != "data" {
            return Err("Unsupported scheme");
        }
        let data_url =
            DataUrl::process(request.url.as_str()).map_err(|_| "Failed to parse data uri")?;
        let (bytes, _) = data_url
            .decode_to_vec()
            .map_err(|_| "Failed to decode data uri")?;
        Ok(bytes)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Mutex;

        /// The handler's completion: `Ok(bytes)` or `Err(message)`.
        type Outcome = Arc<Mutex<Option<Result<Vec<u8>, String>>>>;

        struct Recorder(Outcome);
        impl NetHandler for Recorder {
            fn bytes(self: Box<Self>, _url: String, bytes: Bytes) {
                *self.0.lock().unwrap() = Some(Ok(bytes.to_vec()));
            }
            fn error(self: Box<Self>, _url: String, message: String) {
                *self.0.lock().unwrap() = Some(Err(message));
            }
        }

        fn fetch(url: &str) -> Option<Result<Vec<u8>, String>> {
            let outcome = Outcome::default();
            let request = Request::get(blitz_traits::net::Url::parse(url).unwrap());
            DataUriNetProvider::new(None).fetch(0, request, Box::new(Recorder(outcome.clone())));
            outcome.lock().unwrap().take()
        }

        #[test]
        fn completes_every_request() {
            assert_eq!(fetch("data:text/plain,abc"), Some(Ok(b"abc".to_vec())));
            assert!(matches!(fetch("data:;base64,%%%"), Some(Err(_))));
            assert!(matches!(fetch("https://example.com/a.png"), Some(Err(_))));
        }
    }
}
#[cfg(feature = "data-uri")]
pub use data_uri_net_provider::DataUriNetProvider;

//! The business code: which aircraft pass, what text they become, how the
//! text scrolls and which dots have to flip.
//!
//! Nothing in here parses options, touches a terminal or a pin, or reads a
//! file or the network. It is given frames and values and answers with
//! values, which is what makes it easy to test.
//!
//! From a frame to a flip:
//!
//! - [`tracker`] merges the frames of each aircraft
//! - [`filter`] picks the aircraft inside the rectangle, [`registry`] adds
//!   their registration and type
//! - [`template`] turns an aircraft into lines of text
//! - [`presenter`] decides which aircraft and which text is up
//! - [`scene`] lays the text out and scrolls it
//! - [`dots`] works out which dots differ from what is shown

pub mod dots;
pub mod filter;
pub mod presenter;
pub mod registry;
pub mod scene;
pub mod template;
pub mod tracker;

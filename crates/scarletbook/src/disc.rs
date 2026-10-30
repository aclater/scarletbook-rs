// SPDX-License-Identifier: MIT OR Apache-2.0

//! A disc: an image with its parsed TOC.

use std::io::{Read, Seek};

use crate::error::{Error, Warning};
use crate::image::{Image, SectorFormat};
use crate::toc::{self, Toc};

/// An opened disc image with its table of contents.
///
/// Opening reads only the master TOC and the area TOCs. Audio is read on demand.
#[derive(Debug)]
pub struct Disc<R> {
    image: Image<R>,
    toc: Toc,
    warnings: Vec<Warning>,
}

impl<R: Read + Seek> Disc<R> {
    /// Opens a disc image, detecting its sector format.
    ///
    /// # Errors
    ///
    /// One of the rejections of specification 7, 7.7.1, or [`Error::Io`].
    pub fn open(reader: R) -> Result<Self, Error> {
        Self::from_image(Image::open(reader)?)
    }

    /// Opens a disc image in a sector format chosen by the caller.
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn open_with_format(reader: R, format: SectorFormat) -> Result<Self, Error> {
        Self::from_image(Image::with_format(reader, format)?)
    }

    /// Parses the TOC of an image.
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn from_image(mut image: Image<R>) -> Result<Self, Error> {
        let (toc, warnings) = toc::read_toc(&mut image)?;
        Ok(Disc { image, toc, warnings })
    }

    /// The table of contents.
    pub fn toc(&self) -> &Toc {
        &self.toc
    }

    /// The warnings emitted while reading the TOC (specification 7, 7.7.3).
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The underlying image.
    pub fn image_mut(&mut self) -> &mut Image<R> {
        &mut self.image
    }

    /// Consumes the disc and returns the reader.
    pub fn into_inner(self) -> R {
        self.image.into_inner()
    }
}

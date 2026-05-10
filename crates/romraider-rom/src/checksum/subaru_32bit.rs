//! Subaru 32-bit ECU checksum (SH7058 и аналоги).
//!
//! Портируется из `com.romraider.maps.checksum.ChecksumSubaru32`.

use super::ChecksumModule;
use crate::error::RomResult;
use crate::image::RomImage;

pub struct Subaru32Bit;

impl ChecksumModule for Subaru32Bit {
    fn name(&self) -> &'static str { "subaru_32bit" }

    fn verify(&self, _rom: &RomImage) -> RomResult<()> {
        unimplemented!("Subaru32Bit verify — port from ChecksumSubaru32.java")
    }

    fn fix(&self, _rom: &mut RomImage) -> RomResult<()> {
        unimplemented!("Subaru32Bit fix — port from ChecksumSubaru32.java")
    }
}

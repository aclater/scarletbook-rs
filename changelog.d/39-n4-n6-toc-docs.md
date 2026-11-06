Fixed
- TOC documentation: `Area::slot` and `Area::kind` now say why both exist (the kind comes from the signature, so a slot can yield an area of the other kind), and the module says that TOC LSNs keep their stored `u32` width where `Image` and `seek` use `u64`. No behaviour changes (#39, #323).

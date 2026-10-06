/// Columns file record-layout version. There is no backward compatibility
/// with any other layout: directories written under a different version are
/// rejected at open and must be rebuilt.
pub const COLUMNS_FORMAT_VERSION: u8 = 2;

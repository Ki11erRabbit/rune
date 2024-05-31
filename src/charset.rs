
use std::sync::Mutex;
use std::sync::OnceLock;

use rune_macros::defun;

///Methods for converting code points and characters of charsets.
pub enum Method {
    Offset,
    Map,
    Subset,
    Superset,
}

///Dimension of the charset
pub enum Dimention {
    One,
    Two,
    Three,
    Four,
}

pub struct Charset {
    ///Index to charset_table.
    id: usize,
    ///Index to Vcharset_hash_table.
    hash_index: usize,
    ///Dimension of the charset: 1, 2, 3, or 4.
    dimention: Dimention,
    ///Byte code range of each dimension.  <code_space>[4N] is a minimum
    ///byte code of the (N+1)th dimension, <code_space>[4N+1] is a
    ///maximum byte code of the (N+1)th dimension, <code_space>[4N+2] is
    ///(<code_space>[4N+1] - <code_space>[4N] + 1), <code_space>[4N+3]
    ///is the number of characters contained in the first through (N+1)th
    ///dimensions, except that there is no <code_space>[15].
    ///We get `char-index' of a `code-point' from this
    ///information.
    code_space: [i32; 15],
    ///If B is a byte of Nth dimension of a code-point, the (N-1)th bit
    ///of code_space_mask[B] is set.  This array is used to quickly
    ///check if a code-point is in a valid range. 
    code_space_mask: Vec<u8>,
    ///True if there's no gap in code-points.
    code_linear_p: bool,
    ///True if the charset is treated as 96 chars in ISO-2022
    ///as opposed to 94 chars.
    iso_chars_96: bool,
    ///True if the charset is compatible with ASCII.
    ascii_compatible_p: bool,
    ///True if the charset is supplementary.
    supplementary_p: bool,
    ///True if all the code points are representable by Lisp_Int.
    compact_codes_p: bool,
    ///True if the charset is unified with Unicode.
    unified_p: bool,
    ///ISO final byte of the charset: 48..127.  It may be -1 if the
    ///charset doesn't conform to ISO-2022.
    iso_final: i32,
    ///ISO revision number of the charset.
    iso_revision: i32,
    ///If the charset is identical to what supported by Emacs 21 and the
    ///priors, the identification number of the charset used in those
    ///version.  Otherwise, -1.
    emacs_mule_id: i32,
    ///The method for encoding/decoding characters of the charset. 
    charset_method: Method,
    /// Minimum and Maximum code points of the charset.
    min_code: u32,
    /// Minimum and Maximum code points of the charset.
    max_code: u32,
    ///Offset value used by macros CODE_POINT_TO_INDEX and
    ///INDEX_TO_CODE_POINT.
    char_index_offset: u32,
    ///Minimum and Maximum character codes of the charset.  If the
    ///charset is compatible with ASCII, min_char is a minimum non-ASCII
    ///character of the charset.  If the method of charset is
    ///CHARSET_METHOD_OFFSET, even if the charset is unified, min_char
    ///and max_char doesn't change.
    min_char: u32,
    ///Minimum and Maximum character codes of the charset.  If the
    ///charset is compatible with ASCII, min_char is a minimum non-ASCII
    ///character of the charset.  If the method of charset is
    ///CHARSET_METHOD_OFFSET, even if the charset is unified, min_char
    ///and max_char doesn't change.
    max_char: u32,
    ///The code returned by ENCODE_CHAR if a character is not encodable
    ///by the charset.
    invalid_code: u32,
    ///If the method of the charset is CHARSET_METHOD_MAP, this is a
    ///table of bits used to quickly and roughly guess if a character
    ///belongs to the charset.
    ///
    ///The first 64 elements are 512 bits for characters less than
    ///0x10000.  Each bit corresponds to 128-character block.  The last
    ///126 elements are 1008 bits for the greater characters
    ///(0x10000..0x3FFFFF).  Each bit corresponds to 4096-character
    ///block.
    ///
    ///If a bit is 1, at least one character in the corresponding block is
    ///in this charset.
    fast_map: [u8; 190],
    ///Offset value to calculate a character code from code-point, and
    ///visa versa.
    code_offset: i32,
}

struct Entry {
    from: u32,
    to: u32,
    c: i32,
}

struct MapEntry {
    entry: [Entry; 0x10000],
}


static TABLE: OnceLock<Mutex<Vec<Charset>>> = OnceLock::new();

pub(crate) fn table() -> &'static Mutex<Vec<Charset>> {
    TABLE.get_or_init(Mutex::default)
}

fn load_charset_map(charset: &mut Charset, entries: &mut Vec<MapEntry>, control_flag: i32) {

    let mut vec;
    let mut table;
    let max_code = charset.max_code();
    let ascii_compatible_p = charset.ascii_compatible_p;
    let mut min_char;
    let mut max_char;
    let mut nonascii_min_char;
    let mut i;

    if entries.len() <= 0 {
	return;
    }

    if control_flag {
	if !inhibit_load_charset_map {
	    if control_flag == 1 {
		match charset.charset_method {
		    Method::Map => {
			let n = charset.code_point_to_index(max_code) + 1;
			vec = crate::alloc::make_vector(n, (-1).into());
			charset.set_charset_attr(charset_decoder, vec);
		    }
		    _ => {

		    }
		}
	    }
	}
    }
}

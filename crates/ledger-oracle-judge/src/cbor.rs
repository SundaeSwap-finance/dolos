use thiserror::Error;

/// A transaction output reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxIn {
    pub tx: [u8; 32],
    pub ix: u64,
}

/// A CBOR item that is not the shape a reader asked for.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("cbor at byte {at}: {what}")]
pub struct CborError {
    pub at: usize,
    pub what: &'static str,
}

type Result<T> = std::result::Result<T, CborError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Len {
    Def(u64),
    Indef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    Uint(u64),
    Nint,
    Bytes(Len),
    Text(Len),
    Array(Len),
    Map(Len),
    Tag,
    Simple,
    Break,
}

struct Reader<'b> {
    buf: &'b [u8],
    pos: usize,
}

impl<'b> Reader<'b> {
    fn new(buf: &'b [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn fail<T>(&self, what: &'static str) -> Result<T> {
        Err(CborError { at: self.pos, what })
    }

    fn take(&mut self, n: u64) -> Result<&'b [u8]> {
        let end = usize::try_from(n)
            .ok()
            .and_then(|n| self.pos.checked_add(n))
            .filter(|end| *end <= self.buf.len());
        match end {
            Some(end) => {
                let out = &self.buf[self.pos..end];
                self.pos = end;
                Ok(out)
            }
            None => self.fail("item runs past the end"),
        }
    }

    fn head(&mut self) -> Result<Head> {
        let initial = self.take(1)?[0];
        let (major, info) = (initial >> 5, initial & 0x1f);
        if major == 7 {
            return match info {
                0..=23 => Ok(Head::Simple),
                24 => self.take(1).map(|_| Head::Simple),
                25 => self.take(2).map(|_| Head::Simple),
                26 => self.take(4).map(|_| Head::Simple),
                27 => self.take(8).map(|_| Head::Simple),
                31 => Ok(Head::Break),
                _ => self.fail("reserved simple value"),
            };
        }
        let len = match info {
            0..=23 => Len::Def(u64::from(info)),
            24..=27 => {
                let width = 1u64 << (info - 24);
                let raw = self.take(width)?;
                Len::Def(raw.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
            }
            31 if (2..=5).contains(&major) => Len::Indef,
            _ => return self.fail("reserved additional information"),
        };
        let def = |len: Len| match len {
            Len::Def(n) => Some(n),
            Len::Indef => None,
        };
        Ok(match major {
            0 => Head::Uint(def(len).unwrap_or_default()),
            1 => Head::Nint,
            2 => Head::Bytes(len),
            3 => Head::Text(len),
            4 => Head::Array(len),
            5 => Head::Map(len),
            _ => Head::Tag,
        })
    }

    fn untagged_head(&mut self) -> Result<Head> {
        loop {
            match self.head()? {
                Head::Tag => continue,
                other => return Ok(other),
            }
        }
    }

    fn at_break(&mut self) -> Result<bool> {
        match self.buf.get(self.pos) {
            Some(0xff) => {
                self.pos += 1;
                Ok(true)
            }
            Some(_) => Ok(false),
            None => self.fail("indefinite item runs past the end"),
        }
    }

    fn each(&mut self, len: Len, mut f: impl FnMut(&mut Self) -> Result<()>) -> Result<()> {
        match len {
            Len::Def(n) => (0..n).try_for_each(|_| f(self)),
            Len::Indef => {
                while !self.at_break()? {
                    f(self)?;
                }
                Ok(())
            }
        }
    }

    fn skip(&mut self) -> Result<()> {
        match self.head()? {
            Head::Uint(_) | Head::Nint | Head::Simple => Ok(()),
            Head::Bytes(Len::Def(n)) | Head::Text(Len::Def(n)) => self.take(n).map(|_| ()),
            Head::Bytes(len) | Head::Text(len) | Head::Array(len) => self.each(len, Self::skip),
            Head::Map(len) => self.each(len, |r| {
                r.skip()?;
                r.skip()
            }),
            Head::Tag => self.skip(),
            Head::Break => self.fail("unexpected break"),
        }
    }

    fn array(&mut self) -> Result<Len> {
        match self.untagged_head()? {
            Head::Array(len) => Ok(len),
            _ => self.fail("expected an array"),
        }
    }

    fn map(&mut self) -> Result<Len> {
        match self.untagged_head()? {
            Head::Map(len) => Ok(len),
            _ => self.fail("expected a map"),
        }
    }

    fn uint(&mut self) -> Result<u64> {
        match self.untagged_head()? {
            Head::Uint(n) => Ok(n),
            _ => self.fail("expected an unsigned integer"),
        }
    }

    fn txin(&mut self) -> Result<TxIn> {
        if self.array()? != Len::Def(2) {
            return self.fail("expected a two element input");
        }
        let tx = match self.untagged_head()? {
            Head::Bytes(Len::Def(32)) => {
                let mut tx = [0u8; 32];
                tx.copy_from_slice(self.take(32)?);
                tx
            }
            _ => return self.fail("expected a 32 byte transaction id"),
        };
        Ok(TxIn {
            tx,
            ix: self.uint()?,
        })
    }

    fn txins(&mut self) -> Result<Vec<TxIn>> {
        let len = self.array()?;
        let mut out = Vec::new();
        self.each(len, |r| r.txin().map(|i| out.push(i)))?;
        Ok(out)
    }
}

/// Decodes one output reference from its CBOR bytes.
pub fn txin(raw: &[u8]) -> Result<TxIn> {
    Reader::new(raw).txin()
}

/// The inputs one sub transaction body names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubInputs {
    pub spend: Vec<TxIn>,
    pub reference: Vec<TxIn>,
}

/// The inputs one top level transaction and its sub transactions name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxInputs {
    pub spend: Vec<TxIn>,
    pub collateral: Vec<TxIn>,
    pub reference: Vec<TxIn>,
    pub subs: Vec<SubInputs>,
}

impl TxInputs {
    /// The sub transaction spend inputs that the top level body does not also spend.
    pub fn child_only(&self) -> impl Iterator<Item = &TxIn> {
        self.subs
            .iter()
            .flat_map(|s| &s.spend)
            .filter(|i| !self.spend.contains(i))
    }
}

fn body(r: &mut Reader, mut on: impl FnMut(&mut Reader, u64) -> Result<bool>) -> Result<()> {
    let len = r.map()?;
    r.each(len, |r| {
        let key = r.uint()?;
        if !on(r, key)? {
            r.skip()?;
        }
        Ok(())
    })
}

fn sub_tx(r: &mut Reader) -> Result<SubInputs> {
    let len = r.array()?;
    let mut out = SubInputs::default();
    let mut first = true;
    r.each(len, |r| {
        if !std::mem::take(&mut first) {
            return r.skip();
        }
        body(r, |r, key| {
            match key {
                0 => out.spend = r.txins()?,
                18 => out.reference = r.txins()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    })?;
    Ok(out)
}

/// Reads the inputs a top level transaction and its sub transactions name.
pub fn tx_inputs(raw: &[u8]) -> Result<TxInputs> {
    let mut r = Reader::new(raw);
    let len = r.array()?;
    let mut out = TxInputs::default();
    let mut first = true;
    r.each(len, |r| {
        if !std::mem::take(&mut first) {
            return r.skip();
        }
        body(r, |r, key| {
            match key {
                0 => out.spend = r.txins()?,
                13 => out.collateral = r.txins()?,
                18 => out.reference = r.txins()?,
                23 => {
                    let len = r.array()?;
                    let mut subs = Vec::new();
                    r.each(len, |r| sub_tx(r).map(|s| subs.push(s)))?;
                    out.subs = subs;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    })?;
    match first {
        true => r.fail("expected a transaction body"),
        false => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(tag: bool, byte: u8, ix: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if tag {
            out.extend([0xd9, 0x01, 0x02]);
        }
        out.extend([0x82, 0x58, 0x20]);
        out.extend([byte; 32]);
        out.push(ix);
        out
    }

    #[test]
    fn txin_reads_a_tagged_input_and_refuses_a_short_id() {
        assert_eq!(txin(&input(true, 7, 3)), Ok(TxIn { tx: [7; 32], ix: 3 }));
        let short = [0x82, 0x41, 0x00, 0x00];
        assert_eq!(txin(&short).unwrap_err().what, "expected a 32 byte transaction id");
    }

    #[test]
    fn tx_inputs_reads_tagged_sets_and_sub_bodies() {
        let shared = input(false, 1, 0);
        let own = input(false, 2, 1);
        let mut tx = vec![0x84, 0xa3, 0x00, 0xd9, 0x01, 0x02, 0x81];
        tx.extend(&shared);
        tx.extend([0x02, 0x00, 0x17, 0x81, 0x83, 0xa2, 0x00, 0x82]);
        tx.extend(&shared);
        tx.extend(&own);
        tx.extend([0x12, 0x80, 0xa0, 0xf6, 0xa0, 0xf5, 0xf6]);
        let read = tx_inputs(&tx).unwrap();
        assert_eq!(read.spend, vec![TxIn { tx: [1; 32], ix: 0 }]);
        assert_eq!(read.subs.len(), 1);
        assert_eq!(read.subs[0].spend.len(), 2);
        assert_eq!(
            read.child_only().collect::<Vec<_>>(),
            vec![&TxIn { tx: [2; 32], ix: 1 }]
        );
    }
}

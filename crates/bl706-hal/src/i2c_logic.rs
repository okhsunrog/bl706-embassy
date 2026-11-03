//! FIFO/sub-address bytes go on the wire least-significant byte first.
pub fn pack(bytes: &[u8]) -> u32 {
    assert!(bytes.len() <= 4);
    bytes
        .iter()
        .enumerate()
        .fold(0, |word, (i, b)| word | (u32::from(*b) << (8 * i)))
}

/// Move at most the hardware's two FIFO words per poll, with byte lengths.
pub fn push(
    bytes: &[u8],
    done: &mut usize,
    mut free: impl FnMut() -> u8,
    mut write: impl FnMut(u32),
) {
    for _ in 0..2 {
        if *done == bytes.len() || free() == 0 {
            break;
        }
        let n = core::cmp::min(4, bytes.len() - *done);
        write(pack(&bytes[*done..*done + n]));
        *done += n;
    }
}

pub fn pull(
    bytes: &mut [u8],
    done: &mut usize,
    mut available: impl FnMut() -> u8,
    mut read: impl FnMut() -> u32,
) {
    for _ in 0..2 {
        if *done == bytes.len() || available() == 0 {
            break;
        }
        let word = read().to_le_bytes();
        let n = core::cmp::min(4, bytes.len() - *done);
        bytes[*done..*done + n].copy_from_slice(&word[..n]);
        *done += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sub_address_and_partial_fifo_word_order() {
        assert_eq!(pack(&[0x12, 0x34]), 0x3412);
        assert_eq!(pack(&[0xaa, 0xbb, 0xcc]), 0x00ccbbaa);
        assert_eq!(pack(&[]), 0);
    }
    #[test]
    fn two_word_fifo_backpressure_and_partial_word_lengths() {
        use std::{cell::RefCell, collections::VecDeque};
        for len in 1..=256 {
            let input: Vec<u8> = (0..len).map(|n| n as u8).collect();
            let fifo = RefCell::new(VecDeque::<u32>::new());
            let mut done = 0;
            let mut wire = Vec::new();
            while wire.len() < len {
                push(
                    &input,
                    &mut done,
                    || (2 - fifo.borrow().len()) as u8,
                    |word| {
                        assert!(fifo.borrow().len() < 2, "TX overflow");
                        fifo.borrow_mut().push_back(word);
                    },
                );
                assert!(done <= len);
                // Fake hardware consumes one word per scheduling step.
                let word = fifo.borrow_mut().pop_front().unwrap().to_le_bytes();
                let n = (len - wire.len()).min(4);
                wire.extend_from_slice(&word[..n]);
                assert!(word[n..].iter().all(|b| *b == 0));
            }
            assert_eq!(done, len);
            assert_eq!(input, wire);
            let mut output = vec![0xcc; len];
            done = 0;
            for chunk in input.chunks(4) {
                fifo.borrow_mut().push_back(pack(chunk));
                pull(
                    &mut output,
                    &mut done,
                    || fifo.borrow().len() as u8,
                    || fifo.borrow_mut().pop_front().expect("RX underflow"),
                );
            }
            assert_eq!(done, len);
            assert_eq!(input, output);
            assert!(fifo.borrow().is_empty());
        }
    }

    #[test]
    fn empty_and_full_fifo_do_not_advance_or_touch_data() {
        let mut done = 0;
        push(
            &[1, 2, 3, 4, 5],
            &mut done,
            || 0,
            |_| panic!("write to full FIFO"),
        );
        assert_eq!(done, 0);
        let mut buf = [0xcc; 5];
        pull(&mut buf, &mut done, || 0, || panic!("read empty FIFO"));
        assert_eq!(done, 0);
        assert_eq!(buf, [0xcc; 5]);
    }
}

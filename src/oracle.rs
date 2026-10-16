//! The I Ching oracle: 64 hexagrams in King Wen order, cast per level as the omen.

/// One of the eight trigrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigram {
    Heaven,
    Lake,
    Fire,
    Thunder,
    Wind,
    Water,
    Mountain,
    Earth,
}

impl Trigram {
    /// Lines bottom to top, `true` = yang (heaven 111, lake 110, fire 101, thunder 100,
    /// wind 011, water 010, mountain 001, earth 000, each digit read as bottom, middle, top).
    pub fn lines(self) -> [bool; 3] {
        match self {
            Trigram::Heaven => [true, true, true],
            Trigram::Lake => [true, true, false],
            Trigram::Fire => [true, false, true],
            Trigram::Thunder => [true, false, false],
            Trigram::Wind => [false, true, true],
            Trigram::Water => [false, true, false],
            Trigram::Mountain => [false, false, true],
            Trigram::Earth => [false, false, false],
        }
    }
}

/// A hexagram: number, English name (upper case) and its two trigrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hexagram {
    pub number: u8,
    pub name: &'static str,
    pub upper: Trigram,
    pub lower: Trigram,
}

/// All 64 hexagrams, King Wen order.
pub static HEXAGRAMS: [Hexagram; 64] = [
    Hexagram {
        number: 1,
        name: "THE CREATIVE",
        upper: Trigram::Heaven,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 2,
        name: "THE RECEPTIVE",
        upper: Trigram::Earth,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 3,
        name: "DIFFICULTY AT THE BEGINNING",
        upper: Trigram::Water,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 4,
        name: "YOUTHFUL FOLLY",
        upper: Trigram::Mountain,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 5,
        name: "WAITING",
        upper: Trigram::Water,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 6,
        name: "CONFLICT",
        upper: Trigram::Heaven,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 7,
        name: "THE ARMY",
        upper: Trigram::Earth,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 8,
        name: "HOLDING TOGETHER",
        upper: Trigram::Water,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 9,
        name: "THE TAMING POWER OF THE SMALL",
        upper: Trigram::Wind,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 10,
        name: "TREADING",
        upper: Trigram::Heaven,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 11,
        name: "PEACE",
        upper: Trigram::Earth,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 12,
        name: "STANDSTILL",
        upper: Trigram::Heaven,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 13,
        name: "FELLOWSHIP WITH MEN",
        upper: Trigram::Heaven,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 14,
        name: "POSSESSION IN GREAT MEASURE",
        upper: Trigram::Fire,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 15,
        name: "MODESTY",
        upper: Trigram::Earth,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 16,
        name: "ENTHUSIASM",
        upper: Trigram::Thunder,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 17,
        name: "FOLLOWING",
        upper: Trigram::Lake,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 18,
        name: "WORK ON WHAT HAS BEEN SPOILED",
        upper: Trigram::Mountain,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 19,
        name: "APPROACH",
        upper: Trigram::Earth,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 20,
        name: "CONTEMPLATION",
        upper: Trigram::Wind,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 21,
        name: "BITING THROUGH",
        upper: Trigram::Fire,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 22,
        name: "GRACE",
        upper: Trigram::Mountain,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 23,
        name: "SPLITTING APART",
        upper: Trigram::Mountain,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 24,
        name: "RETURN",
        upper: Trigram::Earth,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 25,
        name: "INNOCENCE",
        upper: Trigram::Heaven,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 26,
        name: "THE TAMING POWER OF THE GREAT",
        upper: Trigram::Mountain,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 27,
        name: "THE CORNERS OF THE MOUTH",
        upper: Trigram::Mountain,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 28,
        name: "PREPONDERANCE OF THE GREAT",
        upper: Trigram::Lake,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 29,
        name: "THE ABYSMAL",
        upper: Trigram::Water,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 30,
        name: "THE CLINGING",
        upper: Trigram::Fire,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 31,
        name: "INFLUENCE",
        upper: Trigram::Lake,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 32,
        name: "DURATION",
        upper: Trigram::Thunder,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 33,
        name: "RETREAT",
        upper: Trigram::Heaven,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 34,
        name: "THE POWER OF THE GREAT",
        upper: Trigram::Thunder,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 35,
        name: "PROGRESS",
        upper: Trigram::Fire,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 36,
        name: "DARKENING OF THE LIGHT",
        upper: Trigram::Earth,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 37,
        name: "THE FAMILY",
        upper: Trigram::Wind,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 38,
        name: "OPPOSITION",
        upper: Trigram::Fire,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 39,
        name: "OBSTRUCTION",
        upper: Trigram::Water,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 40,
        name: "DELIVERANCE",
        upper: Trigram::Thunder,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 41,
        name: "DECREASE",
        upper: Trigram::Mountain,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 42,
        name: "INCREASE",
        upper: Trigram::Wind,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 43,
        name: "BREAK-THROUGH",
        upper: Trigram::Lake,
        lower: Trigram::Heaven,
    },
    Hexagram {
        number: 44,
        name: "COMING TO MEET",
        upper: Trigram::Heaven,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 45,
        name: "GATHERING TOGETHER",
        upper: Trigram::Lake,
        lower: Trigram::Earth,
    },
    Hexagram {
        number: 46,
        name: "PUSHING UPWARD",
        upper: Trigram::Earth,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 47,
        name: "OPPRESSION",
        upper: Trigram::Lake,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 48,
        name: "THE WELL",
        upper: Trigram::Water,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 49,
        name: "REVOLUTION",
        upper: Trigram::Lake,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 50,
        name: "THE CALDRON",
        upper: Trigram::Fire,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 51,
        name: "THE AROUSING",
        upper: Trigram::Thunder,
        lower: Trigram::Thunder,
    },
    Hexagram {
        number: 52,
        name: "KEEPING STILL",
        upper: Trigram::Mountain,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 53,
        name: "DEVELOPMENT",
        upper: Trigram::Wind,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 54,
        name: "THE MARRYING MAIDEN",
        upper: Trigram::Thunder,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 55,
        name: "ABUNDANCE",
        upper: Trigram::Thunder,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 56,
        name: "THE WANDERER",
        upper: Trigram::Fire,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 57,
        name: "THE GENTLE",
        upper: Trigram::Wind,
        lower: Trigram::Wind,
    },
    Hexagram {
        number: 58,
        name: "THE JOYOUS",
        upper: Trigram::Lake,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 59,
        name: "DISPERSION",
        upper: Trigram::Wind,
        lower: Trigram::Water,
    },
    Hexagram {
        number: 60,
        name: "LIMITATION",
        upper: Trigram::Water,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 61,
        name: "INNER TRUTH",
        upper: Trigram::Wind,
        lower: Trigram::Lake,
    },
    Hexagram {
        number: 62,
        name: "PREPONDERANCE OF THE SMALL",
        upper: Trigram::Thunder,
        lower: Trigram::Mountain,
    },
    Hexagram {
        number: 63,
        name: "AFTER COMPLETION",
        upper: Trigram::Water,
        lower: Trigram::Fire,
    },
    Hexagram {
        number: 64,
        name: "BEFORE COMPLETION",
        upper: Trigram::Fire,
        lower: Trigram::Water,
    },
];

/// The hexagram numbered `n`, clamped to 1..=64.
pub fn hexagram(n: u8) -> &'static Hexagram {
    &HEXAGRAMS[usize::from(n.clamp(1, 64)) - 1]
}

/// The six lines bottom to top (`true` = yang); the lower trigram forms the bottom three.
pub fn lines(n: u8) -> [bool; 6] {
    let h = hexagram(n);
    let lo = h.lower.lines();
    let up = h.upper.lines();
    [lo[0], lo[1], lo[2], up[0], up[1], up[2]]
}

/// Display title such as `29 THE ABYSMAL`.
pub fn title(n: u8) -> String {
    let h = hexagram(n);
    format!("{} {}", h.number, h.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn numbers_in_order_and_pairs_unique() {
        for (i, h) in HEXAGRAMS.iter().enumerate() {
            assert_eq!(usize::from(h.number), i + 1);
        }
        let pairs: HashSet<_> = HEXAGRAMS
            .iter()
            .map(|h| (h.upper.lines(), h.lower.lines()))
            .collect();
        assert_eq!(pairs.len(), 64);
    }

    #[test]
    fn spot_checks() {
        assert_eq!(lines(1), [true; 6]);
        assert_eq!(lines(2), [false; 6]);
        assert_eq!(title(29), "29 THE ABYSMAL");
        let h29 = hexagram(29);
        assert_eq!((h29.upper, h29.lower), (Trigram::Water, Trigram::Water));
        let h63 = hexagram(63);
        assert_eq!((h63.upper, h63.lower), (Trigram::Water, Trigram::Fire));
        assert_eq!(lines(11), [true, true, true, false, false, false]);
        assert_eq!(hexagram(9).name, "THE TAMING POWER OF THE SMALL");
    }

    #[test]
    fn clamps_out_of_range() {
        assert_eq!(hexagram(0).number, 1);
        assert_eq!(hexagram(200).number, 64);
    }
}

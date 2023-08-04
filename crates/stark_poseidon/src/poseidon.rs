use stark_curve::FieldElement;

pub type PoseidonState = [FieldElement; 3];
const FULL_ROUNDS: usize = 8;
const PARTIAL_ROUNDS: usize = 83;

include!(concat!(env!("OUT_DIR"), "/poseidon_consts.rs"));

/// Linear layer for MDS matrix M = ((3,1,1), (1,-1,1), (1,1,-2))
/// Given state vector x, it returns Mx, optimized by precomputing t.
#[inline(always)]
fn mix(state: &mut PoseidonState) {
    let t = state[0] + state[1] + state[2];
    state[0] = t + FieldElement::TWO * state[0];
    state[1] = t - FieldElement::TWO * state[1];
    state[2] = t - FieldElement::THREE * state[2];
}

/// Poseidon round function, consist of AddRoundConstants, SubWords and MixLayer
///   - AddRoundConstants adds precomputed constants generated by build.rs
///   - SubWords is the cube function
///   - MixLayer multiplies the state with fixed matrix
#[inline]
#[allow(unused)]
fn round(state: &mut PoseidonState, idx: usize, full: bool) {
    state[0] += POSEIDON_CONSTS[idx];
    state[1] += POSEIDON_CONSTS[idx + 1];
    state[2] += POSEIDON_CONSTS[idx + 2];
    state[2] = state[2] * state[2] * state[2];
    if full {
        state[0] = state[0] * state[0] * state[0];
        state[1] = state[1] * state[1] * state[1];
    }
    mix(state);
}

/// Poseidon permutation function
#[allow(unused)]
pub fn permute(state: &mut PoseidonState) {
    let mut idx = 0;

    // Full rounds
    for _ in 0..(FULL_ROUNDS / 2) {
        round(state, idx, true);
        idx += 3;
    }

    // Partial rounds
    for _ in 0..PARTIAL_ROUNDS {
        round(state, idx, false);
        idx += 3;
    }

    // Full rounds
    for _ in 0..(FULL_ROUNDS / 2) {
        round(state, idx, true);
        idx += 3;
    }
}

#[inline]
fn round_comp(state: &mut PoseidonState, idx: usize, full: bool) {
    if full {
        state[0] += POSEIDON_COMP_CONSTS[idx];
        state[1] += POSEIDON_COMP_CONSTS[idx + 1];
        state[2] += POSEIDON_COMP_CONSTS[idx + 2];
        state[0] = state[0] * state[0] * state[0];
        state[1] = state[1] * state[1] * state[1];
        state[2] = state[2] * state[2] * state[2];
    } else {
        state[2] += POSEIDON_COMP_CONSTS[idx];
        state[2] = state[2] * state[2] * state[2];
    }
    mix(state);
}

/// Poseidon permutation function
pub fn permute_comp(state: &mut PoseidonState) {
    let mut idx = 0;

    // Full rounds
    for _ in 0..(FULL_ROUNDS / 2) {
        round_comp(state, idx, true);
        idx += 3;
    }

    // Partial rounds
    for _ in 0..PARTIAL_ROUNDS {
        round_comp(state, idx, false);
        idx += 1;
    }

    // Full rounds
    for _ in 0..(FULL_ROUNDS / 2) {
        round_comp(state, idx, true);
        idx += 3;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stark_hash::Felt;

    #[test]
    fn test_poseidon() {
        // Test vector from https://github.com/starkware-industries/poseidon
        let hex = [
            "79E8D1E78258000A28FC9D49E233BC6852357968577B1E386550ED6A9086133",
            "3840D003D0F3F96DBB796FF6AA6A63BE5B5404B91CCAABCA256154CBB6FB984",
            "1EB39DA3F7D3B04142D0AC83D9DA00C9325A61FB2EF326E50B70EAA8A3C7CC7",
        ];

        let test_vector = hex.map(|h| FieldElement::from(Felt::from_hex_str(h).unwrap()));

        let mut state: PoseidonState = [FieldElement::ZERO, FieldElement::ZERO, FieldElement::ZERO];
        permute(&mut state);

        assert_eq!(state, test_vector);
    }

    #[test]
    fn test_poseidon_comp() {
        // Test vector from https://github.com/starkware-industries/poseidon
        let hex = [
            "79E8D1E78258000A28FC9D49E233BC6852357968577B1E386550ED6A9086133",
            "3840D003D0F3F96DBB796FF6AA6A63BE5B5404B91CCAABCA256154CBB6FB984",
            "1EB39DA3F7D3B04142D0AC83D9DA00C9325A61FB2EF326E50B70EAA8A3C7CC7",
        ];

        let test_vector = hex.map(|h| FieldElement::from(Felt::from_hex_str(h).unwrap()));

        let mut state: PoseidonState = [FieldElement::ZERO, FieldElement::ZERO, FieldElement::ZERO];
        permute_comp(&mut state);

        assert_eq!(state, test_vector);
    }
}

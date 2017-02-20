// -*- mode: rust; -*-
//
// To the extent possible under law, the authors have waived all copyright and
// related or neighboring rights to curve25519-dalek, using the Creative
// Commons "CC0" public domain dedication.  See
// <http://creativecommons.org/publicdomain/zero/.0/> for full details.
//
// Authors:
// - Isis Agora Lovecruft <isis@patternsinthevoid.net>
// - Henry de Valence <hdevalence@hdevalence.ca>

//! An implementation of Mike Hamburg's Decaf point-compression scheme,
//! providing a prime-order group.

// We allow non snake_case names because coordinates in projective space are
// traditionally denoted by the capitalisation of their respective
// counterparts in affine space.  Yeah, you heard me, rustc, I'm gonna have my
// affine and projective cakes and eat both of them too.
#![allow(non_snake_case)]

use core::fmt::Debug;

use constants;
use field::FieldElement;
use subtle::CTAssignable;

use curve::ExtendedPoint;

// ------------------------------------------------------------------------
// Compressed points
// ------------------------------------------------------------------------

/// A point serialized using Mike Hamburg's Decaf scheme.
///
/// XXX think about how this API should work
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct CompressedDecaf(pub [u8; 32]);

impl CompressedDecaf {
    /// View this `CompressedDecaf` as an array of bytes.
    pub fn to_bytes(&self) -> [u8;32] {
        self.0
    }

    /// Attempt to decompress to an `ExtendedPoint`.
    pub fn decompress(&self) -> Option<ExtendedPoint> {
        // XXX should decoding be CT ?
        // XXX should reject unless s = |s|
        // XXX need to check that xy is nonnegative and reject otherwise
        let s = FieldElement::from_bytes(&self.0);
        let ss = s.square();
        let X = &s + &s;                    // X = 2s
        let Z = &FieldElement::one() - &ss; // Z = 1+as^2
        let u = &(&Z * &Z) - &(&constants::d4 * &ss); // u = Z^2 - 4ds^2
        let uss = &u * &ss;
        let mut v = match uss.invsqrt() {
            Some(v) => v,
            None => return None,
        };
        // Now v = 1/sqrt(us^2) if us^2 is a nonzero square, 0 if us^2 is zero.
        let uv = &v * &u;
        if uv.is_negative_decaf() == 1u8 {
            v.negate();
        }
        let mut two_minus_Z = -&Z; two_minus_Z[0] += 2;
        let mut w = &v * &(&s * &two_minus_Z);
        w.conditional_assign(&FieldElement::one(), s.is_zero());
        let Y = &w * &Z;
        let T = &w * &X;

        Some(ExtendedPoint{ X: X, Y: Y, Z: Z, T: T })
    }
}

impl ExtendedPoint {
    /// Compress in Decaf format.
    pub fn compress_decaf(&self) -> CompressedDecaf {
        // Q: Do we want to encode twisted or untwisted?
        //
        // Notes: 
        // Recall that the twisted Edwards curve E_{a,d} is of the form
        //
        //     ax^2 + y^2 = 1 + dx^2y^2. 
        //
        // Internally, we operate on the curve with a = -1, d =
        // -121665/121666, a.k.a., the twist.  But maybe we would like
        // to use Decaf on the untwisted curve with a = 1, d =
        // 121665/121666.  (why? interop?)
        //
        // Fix i, a square root of -1 (mod p).
        //
        // The map x -> ix is an isomorphism from E_{a,d} to E_{-a,-d}. 
        // Its inverse is x -> -ix.
        // let untwisted_X = &self.X * &constants::MSQRT_M1;
        // etc.

        // Step 0: pre-rotation, needed for Decaf with E[8] = Z/8

        let mut X = self.X;
        let mut Y = self.Y;
        let mut XY = self.T;

        // If y nonzero and xy nonnegative, continue.
        // Otherwise, add Q_6 = (i,0) = constants::EIGHT_TORSION[6]
        // (x,y) + Q_6 = (iy,ix)
        // (X:Y:Z:T) + Q_6 = (iY:iX:Z:-T)

        // XXX it should be possible to avoid this inversion, but
        // let's make sure the code is correct first
        let xy = &XY * &self.Z.invert();
        let is_neg_mask = 1u8 & !(Y.is_nonzero() & xy.is_nonnegative_decaf());
        let iX = &X * &constants::SQRT_M1;
        let iY = &Y * &constants::SQRT_M1;
        X.conditional_assign(&iY, is_neg_mask);
        Y.conditional_assign(&iX, is_neg_mask);
        let minus_XY = -&XY;
        XY.conditional_assign(&minus_XY, is_neg_mask);

        // Step 1: Compute r = 1/sqrt((a-d)(Z+Y)(Z-Y))
        let Z_plus_Y  = &self.Z + &Y;
        let Z_minus_Y = &self.Z - &Y;
        let t = &constants::a_minus_d * &(&Z_plus_Y * &Z_minus_Y);
        // t should always be square (why?)
        // XXX is it safe to use option types here?
        let mut r = t.invsqrt().unwrap();

        // Step 2: Compute u = (a-d)r
        let u = &constants::a_minus_d * &r;

        // Step 3: Negate r if -2uZ is negative.
        let uZ = &u * &self.Z;
        let minus_r = -&r;
        let m2uZ = -&(&uZ + &uZ);
        let mask = m2uZ.is_negative_decaf();
        r.conditional_assign(&minus_r, mask);

        // Step 4: Compute s = |u(r(aZX - dYT)+Y)/a|
        let minus_ZX = -&(&self.Z * &X);
        let dYT = &constants::d * &(&Y * &XY);
        let mut s = &u * &(&(&r * &(&minus_ZX - &dYT)) + &Y);
        s.negate();
        CompressedDecaf(s.abs_decaf().to_bytes())
    }
}


// ------------------------------------------------------------------------
// Debug traits
// ------------------------------------------------------------------------

impl Debug for CompressedDecaf {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        write!(f, "CompressedDecaf: {:?}", &self.0[..])
    }
}


// ------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------

#[cfg(test)]
mod test {
    use rand::OsRng;

    use scalar::Scalar;
    use constants;
    use constants::BASE_CMPRSSD;
    use curve::CompressedEdwardsY;
    use curve::ExtendedPoint;
    use curve::Identity;
    use super::*;

    #[test]
    fn test_decaf_decompress_id() {
        let compressed_id = CompressedDecaf([0u8; 32]);
        let id = compressed_id.decompress().unwrap();
        // This should compress (as ed25519) to the following:
        let mut bytes = [0u8; 32]; bytes[0] = 1;
        assert_eq!(id.compress(), CompressedEdwardsY(bytes));
    }

    #[test]
    fn test_decaf_compress_id() {
        let id = ExtendedPoint::identity();
        assert_eq!(id.compress_decaf(), CompressedDecaf([0u8; 32]));
    }

    #[test]
    fn test_decaf_basepoint_roundtrip() {
        // XXX fix up this test
        let bp = BASE_CMPRSSD.decompress().unwrap();
        let bp_decaf = bp.compress_decaf();
        let bp_recaf = bp_decaf.decompress().unwrap();
        let diff = &bp - &bp_recaf;
        let diff2 = diff.double();
        let diff4 = diff2.double();
        //println!("bp {:?}",       bp);
        //println!("bp_decaf {:?}", bp_decaf);
        //println!("bp_recaf {:?}", bp_recaf);
        //println!("diff {:?}", diff.compress());
        //println!("diff2 {:?}", diff2.compress());
        //println!("diff4 {:?}", diff4.compress());
        assert_eq!(diff4.compress(), ExtendedPoint::identity().compress());
    }

    #[test]
    fn test_decaf_four_torsion_basepoint() {
        //println!("");
        let bp = BASE_CMPRSSD.decompress().unwrap();
        let bp_decaf = bp.compress_decaf();
        //println!("orig, {:?}", bp.compress_decaf());
        for i in (0..8).filter(|x| x % 2 == 0) {
            let Q = &bp + &constants::EIGHT_TORSION[i];
            //println!("{}, {:?}", i, Q.compress_decaf());
            assert_eq!(Q.compress_decaf(), bp_decaf);
        }
    }

    #[test]
    fn test_decaf_four_torsion_random() {
        //println!("");
        let mut rng = OsRng::new().unwrap();
        let s = Scalar::random(&mut rng);
        let P = ExtendedPoint::basepoint_mult(&s);
        let P_decaf = P.compress_decaf();
        //println!("orig, {:?}", P.compress_decaf());
        for i in (0..8).filter(|x| x % 2 == 0) {
            let Q = &P + &constants::EIGHT_TORSION[i];
            //println!("{}, {:?}", i, Q.compress_decaf());
            assert_eq!(Q.compress_decaf(), P_decaf);
        }
    }
}

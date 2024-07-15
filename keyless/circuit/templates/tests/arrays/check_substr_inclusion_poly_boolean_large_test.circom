pragma circom 2.1.3;

include "helpers/arrays.circom";

template check_substr_inclusion_poly_boolean_test(maxStrLen, maxSubstrLen) {
    signal input str[maxStrLen];
    signal input str_hash;
    signal input substr[maxSubstrLen];
    signal input substr_len;
    signal input start_index;
    signal input expected_output;
    
    signal out <== CheckSubstrInclusionPolyBoolean(maxStrLen, maxSubstrLen)(str, str_hash, substr, substr_len, start_index);
    expected_output === out;
}

component main = check_substr_inclusion_poly_boolean_test(
   2000, 1000
);

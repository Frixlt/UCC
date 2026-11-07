use ucc_hardware::requests::*;

#[test]
fn test_ioctl_request_codes_match_driver_abi() {
    // General
    assert_eq!(R_MOD_VERSION, 0x8008ec00, "R_MOD_VERSION mismatch");
    assert_eq!(R_HWCHECK_CL, 0x8008ec05, "R_HWCHECK_CL mismatch");
    assert_eq!(R_HWCHECK_UW, 0x8008ec06, "R_HWCHECK_UW mismatch");

    // Clevo interface - Read
    assert_eq!(R_CL_HW_IF_STR, 0x8008ed00, "R_CL_HW_IF_STR mismatch");
    assert_eq!(R_CL_FANINFO1, 0x8008ed10, "R_CL_FANINFO1 mismatch");
    assert_eq!(R_CL_FANINFO2, 0x8008ed11, "R_CL_FANINFO2 mismatch");
    assert_eq!(R_CL_FANINFO3, 0x8008ed12, "R_CL_FANINFO3 mismatch");
    assert_eq!(R_CL_WEBCAM_SW, 0x8008ed13, "R_CL_WEBCAM_SW mismatch");
    assert_eq!(
        R_CL_FLIGHTMODE_SW, 0x8008ed14,
        "R_CL_FLIGHTMODE_SW mismatch"
    );
    assert_eq!(R_CL_TOUCHPAD_SW, 0x8008ed15, "R_CL_TOUCHPAD_SW mismatch");

    // Clevo interface - Write
    assert_eq!(W_CL_FANSPEED, 0x4008ee10, "W_CL_FANSPEED mismatch");
    assert_eq!(W_CL_FANAUTO, 0x4008ee11, "W_CL_FANAUTO mismatch");
    assert_eq!(W_CL_WEBCAM_SW, 0x4008ee12, "W_CL_WEBCAM_SW mismatch");
    assert_eq!(
        W_CL_FLIGHTMODE_SW, 0x4008ee13,
        "W_CL_FLIGHTMODE_SW mismatch"
    );
    assert_eq!(W_CL_TOUCHPAD_SW, 0x4008ee14, "W_CL_TOUCHPAD_SW mismatch");
    assert_eq!(W_CL_PERF_PROFILE, 0x4008ee15, "W_CL_PERF_PROFILE mismatch");

    // Uniwill interface - Read
    assert_eq!(R_UW_HW_IF_STR, 0x8008ef00, "R_UW_HW_IF_STR mismatch");
    assert_eq!(R_UW_MODEL_ID, 0x8008ef01, "R_UW_MODEL_ID mismatch");
    assert_eq!(R_UW_FANSPEED, 0x8008ef10, "R_UW_FANSPEED mismatch");
    assert_eq!(R_UW_FANSPEED2, 0x8008ef11, "R_UW_FANSPEED2 mismatch");
    assert_eq!(R_UW_FAN_TEMP, 0x8008ef12, "R_UW_FAN_TEMP mismatch");
    assert_eq!(R_UW_FAN_TEMP2, 0x8008ef13, "R_UW_FAN_TEMP2 mismatch");
    assert_eq!(R_UW_MODE, 0x8008ef14, "R_UW_MODE mismatch");
    assert_eq!(R_UW_MODE_ENABLE, 0x8008ef15, "R_UW_MODE_ENABLE mismatch");
    assert_eq!(
        R_UW_FANS_OFF_AVAILABLE, 0x8008ef16,
        "R_UW_FANS_OFF_AVAILABLE mismatch"
    );
    assert_eq!(
        R_UW_FANS_MIN_SPEED, 0x8008ef17,
        "R_UW_FANS_MIN_SPEED mismatch"
    );
    assert_eq!(R_UW_TDP0, 0x8008ef18, "R_UW_TDP0 mismatch");
    assert_eq!(R_UW_TDP1, 0x8008ef19, "R_UW_TDP1 mismatch");
    assert_eq!(R_UW_TDP2, 0x8008ef1a, "R_UW_TDP2 mismatch");
    assert_eq!(R_UW_TDP0_MIN, 0x8008ef1b, "R_UW_TDP0_MIN mismatch");
    assert_eq!(R_UW_TDP1_MIN, 0x8008ef1c, "R_UW_TDP1_MIN mismatch");
    assert_eq!(R_UW_TDP2_MIN, 0x8008ef1d, "R_UW_TDP2_MIN mismatch");
    assert_eq!(R_UW_TDP0_MAX, 0x8008ef1e, "R_UW_TDP0_MAX mismatch");
    assert_eq!(R_UW_TDP1_MAX, 0x8008ef1f, "R_UW_TDP1_MAX mismatch");
    assert_eq!(R_UW_TDP2_MAX, 0x8008ef20, "R_UW_TDP2_MAX mismatch");
    assert_eq!(
        R_UW_PROFS_AVAILABLE, 0x8008ef21,
        "R_UW_PROFS_AVAILABLE mismatch"
    );

    // Uniwill interface - Write
    assert_eq!(W_UW_FANSPEED, 0x4008f010, "W_UW_FANSPEED mismatch");
    assert_eq!(W_UW_FANSPEED2, 0x4008f011, "W_UW_FANSPEED2 mismatch");
    assert_eq!(W_UW_MODE, 0x4008f012, "W_UW_MODE mismatch");
    assert_eq!(W_UW_MODE_ENABLE, 0x4008f013, "W_UW_MODE_ENABLE mismatch");
    assert_eq!(W_UW_FANAUTO, 0x0000f014, "W_UW_FANAUTO mismatch");
    assert_eq!(W_UW_TDP0, 0x4008f015, "W_UW_TDP0 mismatch");
    assert_eq!(W_UW_TDP1, 0x4008f016, "W_UW_TDP1 mismatch");
    assert_eq!(W_UW_TDP2, 0x4008f017, "W_UW_TDP2 mismatch");
    assert_eq!(W_UW_PERF_PROF, 0x4008f018, "W_UW_PERF_PROF mismatch");
}

#[test]
fn test_pointer_size_encoding() {
    // Verify that all read/write ioctls encode 8 bytes (pointer size on 64-bit Linux)
    // and not 4 bytes (size of i32).
    let extract_size = |req: u64| -> u64 { (req >> 16) & 0x3fff };

    assert_eq!(extract_size(R_UW_FANSPEED), 8);
    assert_eq!(extract_size(W_UW_FANSPEED), 8);
    assert_eq!(extract_size(R_MOD_VERSION), 8);
    assert_eq!(extract_size(W_UW_FANAUTO), 0); // _IO has 0 size
}

const OCTAL_BOUND = 010;

typedef float ratio_t;
typedef double measure_t;
typedef quadruple wide_t;

struct full_record_t {
    opaque fixed[OCTAL_BOUND];
    ratio_t *next;
};

union payload_t switch (int kind) {
    case 1:
        int single;
    case 2:
    case 3:
        string text<>;
    default:
        void nothing;
};

enum mode_t {
    MODE_A = 1,
    MODE_B = 2
};

union enum_payload_t switch (mode_t mode) {
    case MODE_A:
        int value;
    default:
        void nothing;
};

typedef enum {
    VALUE_ONE = 1,
    VALUE_TWO = 2
} inline_enum_t;

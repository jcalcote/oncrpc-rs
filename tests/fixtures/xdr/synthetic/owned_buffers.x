const BODY_MAX = 16;
typedef opaque data<BODY_MAX>;
typedef data alias;

struct record {
    float ratio;
    alias value;
};

typedef record *optional_record;
struct wrapper {
    optional_record body;
};

union response switch (bool ok) {
case TRUE:
    wrapper value;
case FALSE:
    void;
};

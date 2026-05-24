%#include "storage/common_types.h"
%#include "storage/nfs_support.h"

typedef unsigned int job_update_t;

struct instance_info_t {
    remote_handle_t source_handle;
};

struct job_result_t {
    status_code_t status;
};

%#include "storage/common_types.h"
%#include "storage/transfer_types.h"

struct copy_request_t {
    job_id_t job_id;
    job_update_t job_update;
    instance_info_t source_instance;
};

program BLOB_SERVICE {
    version BLOB_SERVICE_V1 {
        void BLOB_NULL(void) = 0;
        job_result_t BLOB_COPY(copy_request_t) = 1;
    } = 1;
} = 200001;

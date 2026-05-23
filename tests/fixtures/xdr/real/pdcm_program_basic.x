%#include "pd/pd_types.h"
%#include "pd/pd_dmc_mover_types.h"

struct pdcm_copy_arg_t {
    pdx_job_id_t job_id;
    pddmc_job_update_t job_update;
    pddmc_inst_info_t src_instance;
};

program PDCM_PROGRAM {
    version PDCM_RPC_V8 {
        void PDCM_NULL(void) = 0;
        pddmc_job_res_t PDCM_DO_COPY(pdcm_copy_arg_t) = 1;
    } = 8;
} = 100666;

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PodProvider {
    DataCrunch,
    RunPod,
    VastAi,
    PrimeIntellect,
    AwsEc2,
}

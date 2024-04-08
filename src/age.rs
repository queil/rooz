use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;

pub const VOLUME_NAME: &'static str = "rooz-age-key-vol";

pub fn mount(target: &str) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(VOLUME_NAME.into()),
        target: Some(target.into()),
        ..Default::default()
    }
}

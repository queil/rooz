use crate::{
    api::ConfigApi, config::config::FileFormat, model::types::AnyError
};

impl<'a> ConfigApi<'a> {

    pub async fn template(&self, _format: FileFormat) -> Result<(), AnyError> {
        println!("{}", "# not implemented yet");
        Ok(())
    }
}

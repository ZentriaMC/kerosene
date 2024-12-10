use std::{fs::File, io::ErrorKind, path::Path};

pub fn load_yaml<T>(path: &Path) -> eyre::Result<Option<T>>
where
    T: ::serde::de::DeserializeOwned,
{
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    Ok(Some(serde_yaml::from_reader::<_, T>(file)?))
}

use std::fmt;
use std::path::{Path, PathBuf};

/// A path inside an image, as its names from the root, which has none
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImagePath(Vec<String>);

impl ImagePath {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn join(&self, name: &str) -> Self {
        let mut path = self.clone();
        path.push(name);
        path
    }

    pub fn push(&mut self, name: &str) {
        self.0.push(name.to_owned());
    }

    pub fn parent(&self) -> Option<Self> {
        let (_, parent) = self.0.split_last()?;
        Some(Self(parent.to_vec()))
    }

    pub fn file_name(&self) -> Option<&str> {
        self.0.last().map(String::as_str)
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_os_path(&self, base: &Path) -> PathBuf {
        let mut path = base.to_owned();
        path.extend(self.components());
        path
    }
}

impl From<&str> for ImagePath {
    fn from(path: &str) -> Self {
        let names = path.split('/').filter(|name| !name.is_empty());
        Self(names.map(str::to_owned).collect())
    }
}

impl fmt::Display for ImagePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            return f.write_str("/");
        }

        self.components().try_for_each(|name| write!(f, "/{name}"))
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use super::ImagePath;

    #[test]
    fn test_image_path_parse_display() {
        let path = ImagePath::from("//a//b/");
        assert_eq!(path, ImagePath::root().join("a").join("b"));
        assert_eq!(path.to_string(), "/a/b");

        let root = ImagePath::from("/");
        assert!(root.is_root());
        assert_eq!(root.to_string(), "/");
    }

    #[test]
    fn test_image_path_parent_and_os_path() {
        let path = ImagePath::from("/a/b");
        assert_eq!(path.parent(), Some(ImagePath::from("/a")));
        assert_eq!(ImagePath::root().parent(), None);
        assert_eq!(
            path.to_os_path(Path::new("out")),
            Path::new("out").join("a").join("b")
        );
    }
}

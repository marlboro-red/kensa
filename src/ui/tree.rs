//! Tree building and navigation logic for the file tree.

use std::collections::{HashMap, HashSet};

use super::types::{TreeItem, TreeNode};
use super::App;

impl App {
    /// Invalidate the tree cache (call when files or filters change)
    pub(super) fn invalidate_tree_cache(&mut self) {
        self.cached_tree = None;
        self.cached_flat_items = None;
    }

    /// Ensure flat items cache is populated (builds if needed)
    pub(super) fn ensure_flat_items_cached(&mut self) {
        if self.cached_flat_items.is_none() {
            let tree = self.build_tree_internal();
            let flat_items = self.flatten_tree(&tree);
            self.cached_tree = Some(tree);
            self.cached_flat_items = Some(flat_items);
        }
    }

    /// Get cached flat items for navigation (call ensure_flat_items_cached first)
    pub(super) fn get_flat_items(&self) -> &[TreeItem] {
        self.cached_flat_items.as_deref().unwrap_or(&[])
    }

    /// Collect all folder paths from the current files
    pub(super) fn collect_folder_paths(&self) -> HashSet<String> {
        let mut folders = HashSet::new();
        for file in &self.files {
            let parts: Vec<&str> = file.path.split('/').collect();
            // Build all folder paths (all but the last part which is the filename)
            for i in 1..parts.len() {
                let folder_path = parts[..i].join("/");
                folders.insert(folder_path);
            }
        }
        folders
    }

    /// Initialize collapsed_folders based on config setting
    pub(super) fn init_collapsed_folders(&mut self) {
        if self.config.navigation.collapse_folders_by_default {
            self.collapsed_folders = self.collect_folder_paths();
            // Select the first root folder so the user can navigate
            let mut root_folders: Vec<_> = self
                .files
                .iter()
                .filter(|f| f.path.contains('/'))
                .filter_map(|f| f.path.split('/').next())
                .collect();
            root_folders.sort();
            root_folders.dedup();
            self.selected_tree_item = root_folders.first().map(|s| s.to_string());
        } else {
            self.collapsed_folders.clear();
            self.selected_tree_item = None;
        }
    }

    /// Build a tree structure from the flat file list (used by caching system)
    fn build_tree_internal(&self) -> Vec<TreeNode> {
        let mut root: HashMap<String, TreeNode> = HashMap::new();

        // Only include filtered files
        for &file_idx in &self.filtered_indices {
            let file = &self.files[file_idx];
            let parts: Vec<&str> = file.path.split('/').collect();

            if parts.len() == 1 {
                // File at root level
                root.insert(
                    file.path.clone(),
                    TreeNode::File {
                        name: file.path.clone(),
                        index: file_idx,
                    },
                );
            } else {
                // File in a subdirectory - build path
                self.insert_into_tree(&mut root, &parts, file_idx);
            }
        }

        // Convert HashMap to sorted Vec
        let mut nodes: Vec<TreeNode> = root.into_values().collect();
        self.sort_tree_nodes(&mut nodes);
        nodes
    }

    fn insert_into_tree(
        &self,
        root: &mut HashMap<String, TreeNode>,
        parts: &[&str],
        file_idx: usize,
    ) {
        if parts.is_empty() {
            return;
        }

        let first = parts[0];

        if parts.len() == 1 {
            // This is a file
            root.insert(
                first.to_string(),
                TreeNode::File {
                    name: first.to_string(),
                    index: file_idx,
                },
            );
        } else {
            // This is a folder
            let first_folder_path = first.to_string();

            let folder = root
                .entry(first.to_string())
                .or_insert_with(|| TreeNode::Folder {
                    name: first.to_string(),
                    path: first_folder_path,
                    children: Vec::new(),
                });

            if let TreeNode::Folder { children, path, .. } = folder {
                // Update path to be the full path up to this folder
                if parts.len() > 1 {
                    *path = parts[0].to_string();
                }
                let mut child_map: HashMap<String, TreeNode> = children
                    .drain(..)
                    .map(|n| {
                        let key = match &n {
                            TreeNode::Folder { name, .. } => name.clone(),
                            TreeNode::File { name, .. } => name.clone(),
                        };
                        (key, n)
                    })
                    .collect();

                self.insert_into_tree_nested(&mut child_map, &parts[1..], file_idx, &parts[..1]);

                *children = child_map.into_values().collect();
            }
        }
    }

    fn insert_into_tree_nested(
        &self,
        parent: &mut HashMap<String, TreeNode>,
        parts: &[&str],
        file_idx: usize,
        prefix: &[&str],
    ) {
        if parts.is_empty() {
            return;
        }

        let first = parts[0];

        if parts.len() == 1 {
            // This is a file
            parent.insert(
                first.to_string(),
                TreeNode::File {
                    name: first.to_string(),
                    index: file_idx,
                },
            );
        } else {
            // This is a folder
            let mut full_path_parts: Vec<&str> = prefix.to_vec();
            full_path_parts.push(first);
            let folder_path = full_path_parts.join("/");

            let folder = parent
                .entry(first.to_string())
                .or_insert_with(|| TreeNode::Folder {
                    name: first.to_string(),
                    path: folder_path.clone(),
                    children: Vec::new(),
                });

            if let TreeNode::Folder { children, .. } = folder {
                let mut child_map: HashMap<String, TreeNode> = children
                    .drain(..)
                    .map(|n| {
                        let key = match &n {
                            TreeNode::Folder { name, .. } => name.clone(),
                            TreeNode::File { name, .. } => name.clone(),
                        };
                        (key, n)
                    })
                    .collect();

                self.insert_into_tree_nested(
                    &mut child_map,
                    &parts[1..],
                    file_idx,
                    &full_path_parts,
                );

                *children = child_map.into_values().collect();
            }
        }
    }

    fn sort_tree_nodes(&self, nodes: &mut [TreeNode]) {
        nodes.sort_by(|a, b| {
            match (a, b) {
                // Folders come before files
                (TreeNode::Folder { name: a, .. }, TreeNode::Folder { name: b, .. }) => a.cmp(b),
                (TreeNode::File { name: a, .. }, TreeNode::File { name: b, .. }) => a.cmp(b),
                (TreeNode::Folder { .. }, TreeNode::File { .. }) => std::cmp::Ordering::Less,
                (TreeNode::File { .. }, TreeNode::Folder { .. }) => std::cmp::Ordering::Greater,
            }
        });

        for node in nodes.iter_mut() {
            if let TreeNode::Folder { children, .. } = node {
                self.sort_tree_nodes(children);
            }
        }
    }

    /// Flatten the tree into a list of items for rendering
    pub(super) fn flatten_tree(&self, nodes: &[TreeNode]) -> Vec<TreeItem> {
        let mut items = Vec::new();
        self.flatten_tree_recursive(nodes, 0, &mut items, &[]);
        items
    }

    fn flatten_tree_recursive(
        &self,
        nodes: &[TreeNode],
        depth: usize,
        items: &mut Vec<TreeItem>,
        ancestors_last: &[bool],
    ) {
        let len = nodes.len();
        for (i, node) in nodes.iter().enumerate() {
            let is_last = i == len - 1;
            let mut current_ancestors: Vec<bool> = ancestors_last.to_vec();

            match node {
                TreeNode::Folder {
                    name,
                    path,
                    children,
                } => {
                    items.push(TreeItem::Folder {
                        path: path.clone(),
                        name: name.clone(),
                        depth,
                        is_last,
                        ancestors_last: current_ancestors.clone(),
                    });

                    // Only show children if folder is expanded
                    if !self.collapsed_folders.contains(path) {
                        current_ancestors.push(is_last);
                        self.flatten_tree_recursive(children, depth + 1, items, &current_ancestors);
                    }
                }
                TreeNode::File { name, index } => {
                    items.push(TreeItem::File {
                        index: *index,
                        name: name.clone(),
                        depth,
                        is_last,
                        ancestors_last: current_ancestors,
                    });
                }
            }
        }
    }

    /// Get the tree prefix characters for a given depth and position
    pub(super) fn get_tree_prefix(
        &self,
        _depth: usize,
        is_last: bool,
        ancestors_last: &[bool],
    ) -> String {
        let mut prefix = String::new();

        for &ancestor_is_last in ancestors_last {
            if ancestor_is_last {
                prefix.push_str("  ");
            } else {
                prefix.push_str("│ ");
            }
        }

        // Add tree branch characters for all items
        if is_last {
            prefix.push_str("└─");
        } else {
            prefix.push_str("├─");
        }

        prefix
    }
}

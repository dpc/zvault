use prelude::*;

use std::collections::HashSet;


impl Repository {
    fn delete_bundle(&mut self, id: u32) -> Result<(), RepositoryError> {
        if let Some(bundle) = self.bundle_map.remove(id) {
            try!(self.bundles.delete_bundle(&bundle));
            Ok(())
        } else {
            Err(IntegrityError::MissingBundleId(id).into())
        }
    }

    pub fn vacuum(
        &mut self,
        ratio: f32,
        combine: bool,
        force: bool,
    ) -> Result<(), RepositoryError> {
        try!(self.flush());
        info!("Locking repository");
        try!(self.write_mode());
        let _lock = try!(self.lock(true));
        // analyze_usage will set the dirty flag
        info!("Analyzing chunk usage");
        let usage = try!(self.analyze_usage());
        let mut data_total = 0;
        let mut data_used = 0;
        for bundle in usage.values() {
            data_total += bundle.info.encoded_size;
            data_used += bundle.get_used_size();
        }
        info!(
            "Usage: {} of {}, {:.1}%",
            to_file_size(data_used as u64),
            to_file_size(data_total as u64),
            data_used as f32 / data_total as f32 * 100.0
        );
        let mut rewrite_bundles = HashSet::new();
        let mut reclaim_space = 0;
        for (id, bundle) in &usage {
            if bundle.get_usage_ratio() <= ratio {
                rewrite_bundles.insert(*id);
                reclaim_space += bundle.get_unused_size();
            }
        }
        if combine {
            let mut small_meta = vec![];
            let mut small_data = vec![];
            for (id, bundle) in &usage {
                if bundle.info.encoded_size * 4 < self.config.bundle_size {
                    match bundle.info.mode {
                        BundleMode::Meta => small_meta.push(*id),
                        BundleMode::Data => small_data.push(*id),
                    }
                }
            }
            if small_meta.len() >= 2 {
                for bundle in small_meta {
                    rewrite_bundles.insert(bundle);
                }
            }
            if small_data.len() >= 2 {
                for bundle in small_data {
                    rewrite_bundles.insert(bundle);
                }
            }
        }
        info!(
            "Reclaiming {} by rewriting {} bundles",
            to_file_size(reclaim_space as u64),
            rewrite_bundles.len()
        );
        if !force {
            self.dirty = false;
            return Ok(());
        }
        for id in ProgressIter::new(
            "rewriting bundles",
            rewrite_bundles.len(),
            rewrite_bundles.iter()
        )
        {
            let bundle = &usage[id];
            let bundle_id = self.bundle_map.get(*id).unwrap();
            let chunks = try!(self.bundles.get_chunk_list(&bundle_id));
            let mode = usage[id].info.mode;
            for (chunk, &(hash, _len)) in chunks.into_iter().enumerate() {
                if !bundle.chunk_usage.get(chunk) {
                    try!(self.index.delete(&hash));
                    continue;
                }
                let data = try!(self.bundles.get_chunk(&bundle_id, chunk));
                try!(self.put_chunk_override(mode, hash, &data));
            }
        }
        try!(self.flush());
        info!("Checking index");
        for (hash, location) in self.index.iter() {
            if rewrite_bundles.contains(&location.bundle) {
                panic!(
                    "Removed bundle is still referenced in index: hash:{}, bundle:{}, chunk:{}",
                    hash,
                    location.bundle,
                    location.chunk
                );
            }
        }
        info!("Deleting {} bundles", rewrite_bundles.len());
        for id in rewrite_bundles {
            try!(self.delete_bundle(id));
        }
        try!(self.save_bundle_map());
        self.dirty = false;
        Ok(())
    }
}

//! Parça deposunun dış hedefle eşitlenmesi: gönderme, geri indirme, hedeften silme.

use super::*;

impl Store {
    /// Hedefteki bir dizini indirip doğrular; andığı parçaları uzunluklarıyla döndürür.
    pub(super) fn remote_chunks(&self, id: &str, remote: &dyn Remote) -> Result<(Vec<(String, usize)>, PathBuf), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("yedek dizini oluşturulamadı: {e}"))?;
        let staged = self.dir.join(format!(".{id}.{}-{}.rdxi.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
        let parsed = remote.get(&remote_index(id), &staged).and_then(|()| {
            let text = std::fs::read(&staged).map_err(|e| format!("indirilen dizin okunamadı: {e}"))?;
            let index = Self::parse(&text).and_then(|index| Self::owned_by(index, id)).map_err(|e| format!("dış hedefteki yedek dizini geçersiz ({id}): {e}"))?;
            Ok(Self::unique_chunks(&index))
        });
        match parsed {
            Ok(chunks) => Ok((chunks, staged)),
            Err(e) => {
                let _ = std::fs::remove_file(&staged);
                Err(e)
            }
        }
    }

    /// Yedeği dış hedefe gönderir: yalnızca hedefte olmayan parçalar gider, dizin en son gönderilir.
    /// `known`: dış kopyası olduğu bilinen diğer yedekler; onların parçaları hedefe sorulmadan atlanır.
    /// `(gönderilen parça sayısı, gönderilen bayt)` döndürür.
    pub fn push(&self, id: &str, known: &[String], remote: &dyn Remote) -> Result<(usize, u64), String> {
        let _sync = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let index = self.load(id)?;
        self.check_remote_store(remote)?;
        let mut there: HashSet<String> = HashSet::new();
        for other in known.iter().filter(|other| other.as_str() != id) {
            // Yerelde dizini olmayan (ya da okunamayan) yedek bilgi vermez; parçaları hedefe sorulur.
            // Kayıttaki "dış kopyası var" işaretine körü körüne güvenilmez: hedef değiştirilmiş ya da
            // boşaltılmış olabilir. Yedeğin dizini hedefte gerçekten duruyorsa parçaları da oradadır.
            if let Ok(index) = self.load(other) {
                if remote.exists(&remote_index(other))? {
                    there.extend(Self::unique_chunks(&index).into_iter().map(|(hash, _)| hash));
                }
            }
        }
        let (mut sent, mut bytes) = (0usize, 0u64);
        for (hash, _) in Self::unique_chunks(&index) {
            if there.contains(&hash) {
                continue;
            }
            let name = remote_chunk(&hash);
            if !remote.exists(&name)? {
                bytes += remote.put(&name, &self.chunk_path(&hash))?;
                sent += 1;
            }
        }
        // Dizin en son: hedefte dizini olan yedeğin bütün parçaları da hedeftedir.
        bytes += remote.put(&remote_index(id), &self.index_path(id))?;
        Ok((sent, bytes))
    }

    /// Hedefteki depo işareti: ilk göndermede konur, sonrakilerde okunur. Şifreleme parolası değişmişse
    /// okunamaz; böylece eski parolayla şifrelenmiş parçalar "hedefte var" sayılıp yeni yedeğin dış kopyası
    /// açılamaz halde bırakılmaz.
    pub(super) fn check_remote_store(&self, remote: &dyn Remote) -> Result<(), String> {
        const MARKER: &str = "redox-store.json";
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("yedek dizini oluşturulamadı: {e}"))?;
        let probe = self.dir.join(format!(".store.{}-{}.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
        let checked = if remote.exists(MARKER)? {
            remote.get(MARKER, &probe).map_err(|e| {
                format!("dış hedefteki yedek deposu açılamıyor; şifreleme parolası değiştiyse eski parolayı geri yazın ya da boş bir hedef kullanın: {e}")
            })
        } else {
            std::fs::write(&probe, b"{\"redox\":1}\n").map_err(|e| format!("{} yazılamadı: {e}", probe.display())).and_then(|()| remote.put(MARKER, &probe).map(|_| ()))
        };
        let _ = std::fs::remove_file(&probe);
        checked
    }

    /// Dış hedefteki yedeği yerel depoya indirir: yerelde olmayan ya da bozulmuş parçalar gelir ve her biri
    /// yerine konmadan önce özetiyle doğrulanır. Hedefte bu yedeğin dizini yoksa `false`.
    pub fn pull(&self, id: &str, remote: &dyn Remote) -> Result<bool, String> {
        // Hedefte silme (`unpush`) ile aynı anda çalışmaz: yoksa silme, az sonra yerele inecek yedeğin
        // parçalarını "kimse kullanmıyor" sanabilirdi.
        let _sync = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if !remote.exists(&remote_index(id))? {
            return Ok(false);
        }
        let _gate = self.gate.read().unwrap_or_else(|e| e.into_inner());
        let (chunks, staged) = self.remote_chunks(id, remote)?;
        let fetched = (|| {
            let mut touched: HashSet<PathBuf> = HashSet::new();
            for (hash, len) in &chunks {
                let path = self.chunk_path(hash);
                // Yerelde duran parça da doğrulanır: bozulmuşsa hedefteki sağlam kopyayla değiştirilir.
                if path.is_file() && Self::decode(&path, hash, *len).is_ok() {
                    continue;
                }
                let parent = path.parent().ok_or("geçersiz parça yolu")?;
                std::fs::create_dir_all(parent).map_err(|e| format!("{} oluşturulamadı: {e}", parent.display()))?;
                let temp = parent.join(format!(".{hash}.{}-{}.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
                let checked = remote.get(&remote_chunk(hash), &temp).and_then(|()| Self::decode(&temp, hash, *len).map(|_| ()));
                let placed = checked.and_then(|()| std::fs::rename(&temp, &path).map_err(|e| format!("parça yerine konamadı: {e}")));
                if let Err(e) = placed {
                    let _ = std::fs::remove_file(&temp);
                    return Err(format!("dış kopya indirilemedi: {e}"));
                }
                touched.insert(parent.to_path_buf());
            }
            self.settle(&touched);
            // Dizin en son yerine konur: yarım kalan indirme geçerli bir yedek gibi görünmez.
            std::fs::rename(&staged, self.index_path(id)).map_err(|e| format!("yedek dizini yerine konamadı: {e}"))
        })();
        if let Err(e) = fetched {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
        sync_dir(&self.dir);
        Ok(true)
    }

    /// Dış hedefteki yedeği siler: önce dizini, sonra yalnızca ona özgü parçaları. Korunan parçalar:
    /// yereldeki bütün dizinlerin ve `others` içinde yalnızca hedefte kalmış yedeklerin andıkları.
    /// Bunlar belirlenemezse hiçbir parça silinmez (hedefte artık parça kalır, veri kaybı olmaz).
    /// Hedefte bu yedeğin dizini yoksa `false`.
    pub fn unpush(&self, id: &str, others: &[String], remote: &dyn Remote) -> Result<bool, String> {
        let _sync = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if !remote.exists(&remote_index(id))? {
            return Ok(false);
        }
        let (victims, staged) = self.remote_chunks(id, remote)?;
        let _ = std::fs::remove_file(&staged);
        remote.delete(&remote_index(id))?;
        let keep = (|| {
            let mut keep = self.referenced()?;
            for other in others.iter().filter(|other| other.as_str() != id && !self.has(other)) {
                // Tam (tek dosyalık) dış kopyaların dizini yoktur; onlar parça kullanmaz.
                if remote.exists(&remote_index(other))? {
                    let (chunks, staged) = self.remote_chunks(other, remote)?;
                    let _ = std::fs::remove_file(&staged);
                    keep.extend(chunks.into_iter().map(|(hash, _)| hash));
                }
            }
            Ok::<_, String>(keep)
        })()
        .map_err(|e| format!("dış kopyanın dizini silindi ama parçaları hedefte bırakıldı (hangilerinin gerekli olduğu belirlenemedi): {e}"))?;
        for (hash, _) in victims {
            if !keep.contains(&hash) {
                remote.delete(&remote_chunk(&hash))?;
            }
        }
        Ok(true)
    }
}

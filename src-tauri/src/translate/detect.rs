//! 本地语言检测（规格 04 §7）：先本地判断，省一次网络请求。结果只是默认值，界面上可改。

/// 只在简体里出现的常用字、只在繁体里出现的常用字（各两三百个）。中文按哪边的字多分简繁
const SIMPLIFIED_ONLY: &str = "这个们来时说对会过还从么为国学开关问题实体点头东车马门见长电话语发现经让应该觉当变认处义区书买卖写听读观欢显钱样边动结乐际传务旧广华选择号据队级导总环张无术吗风飞鸟鱼龙红绿蓝黄万与业丝两严临举习乡乱争亏亚产亲亿仅仓仪价众优伟伤伦侦债倾储儿党兰兴养册军农决况净减凤击刘则刚创删别剧劝办励劳势医协单卫却厂历厅压县参双叹吓启员响团园围图圆场坏块坚备复够夹夺奋奖妇妈宁宝审宽宾寻将尔尘层属岁岛币师带帮庆库废异弃弯弹归录忆忧怀态恋恶惊愿戏战扩扫扬护报担拥挂挡挤挥损换摄摆敌数断晋暂机杀杂权条杨极构枪档桥检楼标树欧毕气汇汉汤没沟泪泽洁浅测济浓涂润涨渐温湾湿满灭灯灵灾炉烟热爱状独猎猪献画疗盐监盖盘础确礼离种积称稳穷竞笔签简类粮紧约纪纯纸线练组细织终给络统继续维编网罗职联脑脚脸节艺苏药获营虑虚虽补装规视览计订讨训议记讲许论设访证评识诉词译试诗诚询详误请课谁调谈谢贝负财责败货质购贵费资赛赶趋跃践转轮软轻载较辆达迁运进远违连迟递遗邮释针钟钢铁银链销锁错键镇闭闲间闻阅阳阴阵阶陆陈险随隐难雾韩页顶项顺须顾顿预领频颜额饭饮馆驾验骑鲁鲜鸡麦齐龄";
const TRADITIONAL_ONLY: &str = "這個們來時說對會後過還從麼為國學開關問題實體點頭東車馬門見長電話語發現經裡讓應該覺當變認處義區書買賣寫聽讀觀歡顯錢樣邊動結樂際傳務舊廣華選擇號據隊級導總環張無術嗎風飛鳥魚龍紅綠藍黃萬與業絲兩嚴臨舉習鄉亂爭虧雲亞產親億僅倉儀價眾優偉傷倫餘偵債傾儲兒黨蘭興養冊軍農衝決況淨準減幾鳳擊劉則剛創刪別劇勸辦勵勞勢醫協單衛卻廠歷廳壓縣參雙葉嘆嚇啟員響團園圍圖圓場壞塊堅備復夠夾奪奮獎婦媽寧寶審寬賓尋將爾塵層屬歲島幣師帶幫慶庫廢異棄彎彈歸錄憶憂懷態戀惡驚願戲戰擴掃揚護報擔擁掛擋擠揮損換攝擺敵數斷晉暫機殺雜權條楊極構槍檔橋檢樓標樹歐畢氣匯漢湯沒溝淚澤潔淺測濟濃塗潤漲漸溫灣濕滿滅燈靈災爐煙熱愛狀獨獵豬獻畫療鹽監蓋盤礎確禮離種積稱穩窮競筆簽簡類糧緊約紀純紙線練組細織終給絡統繼續維編網羅職聯腦腳臉節藝蘇藥獲營慮虛雖補裝規視覽計訂討訓議記講許論設訪證評識訴詞譯試詩誠詢詳誤請課誰調談謝貝負財責敗貨質購貴費資賽趕趨躍踐轉輪軟輕載較輛達遷運進遠違連遲遞遺郵釋針鐘鋼鐵銀鏈銷鎖錯鍵鎮閉閒間聞閱陽陰陣階陸陳險隨隱難霧韓頁頂項順須顧頓預領頻顏額飯飲館駕驗騎魯鮮雞麥齊齡";

/// 中文是繁体（zh-TW）还是简体（zh）：两边独有的字哪边多。都没有（"中文"这种两边一样的字）算简体
fn chinese_variant(text: &str) -> &'static str {
    let (mut simp, mut trad) = (0, 0);
    for c in text.chars() {
        if SIMPLIFIED_ONLY.contains(c) {
            simp += 1;
        } else if TRADITIONAL_ONLY.contains(c) {
            trad += 1;
        }
    }
    if trad > simp {
        "zh-TW"
    } else {
        "zh"
    }
}

pub fn detect(text: &str) -> &'static str {
    let mut total = 0usize;
    let (mut han, mut kana, mut hangul, mut cyrillic, mut arabic, mut thai) = (0, 0, 0, 0, 0, 0);
    for c in text.chars().filter(|c| c.is_alphanumeric()) {
        total += 1;
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana += 1,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x0400..=0x04FF => cyrillic += 1,
            0x0600..=0x06FF => arabic += 1,
            0x0E00..=0x0E7F => thai += 1,
            _ => {}
        }
    }
    if total == 0 {
        return "en";
    }
    let ratio = |n: usize| n as f64 / total as f64;
    let cjk = han + kana + hangul;
    if ratio(cjk) > 0.2 {
        // 有假名就是日文（日文里也大量用汉字）；有谚文就是韩文
        if kana > 0 && ratio(kana) > 0.05 {
            return "ja";
        }
        if hangul > han {
            return "ko";
        }
        return chinese_variant(text);
    }
    if ratio(cyrillic) > 0.2 {
        return "ru";
    }
    if arabic > 0 && ratio(arabic) > 0.2 {
        return "ar";
    }
    if thai > 0 && ratio(thai) > 0.2 {
        return "th";
    }
    "en"
}

/// 默认目标语言：中文 → 英文，其他 → 中文。
pub fn default_target(source: &str) -> &'static str {
    if source == "zh" || source == "zh-TW" {
        "en"
    } else {
        "zh"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_languages() {
        assert_eq!(detect("这是一段中文 with some English"), "zh");
        assert_eq!(detect("這是一段繁體中文的說明，請選擇語言"), "zh-TW");
        assert_eq!(detect("这是一段简体中文的说明，请选择语言"), "zh");
        assert_eq!(detect("中文"), "zh");
        assert_eq!(detect("The quick brown fox"), "en");
        assert_eq!(detect("これは日本語の文章です"), "ja");
        assert_eq!(detect("안녕하세요 반갑습니다"), "ko");
        assert_eq!(detect("Привет, как дела?"), "ru");
        assert_eq!(detect("12345 !!!"), "en");
        assert_eq!(default_target("zh"), "en");
        assert_eq!(default_target("ja"), "zh");
    }
}

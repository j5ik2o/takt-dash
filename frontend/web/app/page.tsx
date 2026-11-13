import Image from "next/image";
import styles from "./page.module.css";

export default function Home() {
  return (
    <div className={styles.page}>
      <main className={styles.main}>
        <Image
          className={styles.logo}
          src="/next.svg"
          alt="Next.js のロゴ"
          width={100}
          height={20}
          priority
        />
        <div className={styles.intro}>
          <h1>
            開発者は、{" "}
            <code className={styles.code}>app/page.tsx</code>{" "}
            ファイルを編集して、この画面の開発を始めます。
          </h1>
          <p>
            開発の手始めや詳しい手順を探している人は、{" "}
            <a
              href="https://vercel.com/templates?framework=next.js&utm_source=create-next-app&utm_medium=appdir-template-tw&utm_campaign=create-next-app"
              target="_blank"
              rel="noopener noreferrer"
            >
              テンプレート
            </a>{" "}
            または{" "}
            <a
              href="https://nextjs.org/learn?utm_source=create-next-app&utm_medium=appdir-template-tw&utm_campaign=create-next-app"
              target="_blank"
              rel="noopener noreferrer"
            >
              学習センター
            </a>{" "}
            を参照してください。
          </p>
        </div>
        <div className={styles.ctas}>
          <a
            className={styles.primary}
            href="https://vercel.com/new?utm_source=create-next-app&utm_medium=appdir-template&utm_campaign=create-next-app"
            target="_blank"
            rel="noopener noreferrer"
          >
            <Image
              className={styles.logo}
              src="/vercel.svg"
              alt="Vercel のロゴマーク"
              width={16}
              height={14}
            />
            今すぐデプロイする
          </a>
          <a
            className={styles.secondary}
            href="https://nextjs.org/docs?utm_source=create-next-app&utm_medium=appdir-template&utm_campaign=create-next-app"
            target="_blank"
            rel="noopener noreferrer"
          >
            ドキュメントを読む
          </a>
        </div>
      </main>
    </div>
  );
}
